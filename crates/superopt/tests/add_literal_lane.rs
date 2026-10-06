//! Combined 16/32-bit literal add/sub lanes (epic-cc#767): `x += k` and
//! `x -= k` over two and four bytes, computed in place (same addresses in
//! and out, matching the store-folded and phi-folded `isel-pic18` sites).
//!
//! `isel-pic18`'s current lowering stages every lane through `W`
//! (`MOVLW kb` + `ADDWF`/`ADDWFC f,W` + `MOVWF`), 3 words per lane, so the
//! baselines to beat are **6 words** (16-bit) and **12 words** (32-bit).
//! Every candidate here writes its result back into the source bytes, so
//! the out-of-place baseline applies unchanged: it reads each lane before
//! writing it, which stays correct when source and destination coincide.
//!
//! Two independent result sources, mirroring `shift_16bit.rs`:
//!
//! - **Bounded exhaustive search** (`shortest`, beat-baseline assert) for
//!   16-bit plus/minus one over a curated alphabet, the only targets whose
//!   optimum sits within a cheap bound (length 3 over ~13 symbols).
//! - **Constructed candidates**, each `verify()`d over the full 16-bit
//!   domain (single `W`, single `C`, like `shift_16bit.rs`' exhaustive arm)
//!   plus a curated sample across `W_SAMPLE` and both `C` values, and over
//!   a structured 32-bit word sample. Generic-`k` shapes are verified at
//!   representative literals; the `LITERALS` tables are what check the
//!   shape generalizes beyond any one of them.
//!
//! Every construction preserves `W` except the generic `MOVLW` lane forms
//! (which must stage the literal through it, like the baseline). `W` is
//! scratch to `verify` by design, so this is stated here instead: the
//! `isel-pic18` sites these land at must have `W` dead, which holds at
//! every `Bin` emission (no lowering reads `W` before writing it).
//!
//! Registers: `LO` = 0x020, `HI` = 0x021 (`B2` = 0x022, `B3` = 0x023 for
//! 32-bit), little-endian, arbitrary access-bank GPRs.

use pic14_sim::Pic18;
use superopt::{shortest, verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LO: usize = 0x020;
const HI: usize = 0x021;
const B2: usize = 0x022;
const B3: usize = 0x023;

const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Plus/minus-one cases also assert `W` survives (no literal to stage);
/// the generic lane forms below check the bytes only, like the baseline
/// they replace (which leaves the last literal byte in `W`).
fn cases16_over(
    expect: impl Fn(u16) -> u16 + Copy,
    pairs: &[(u8, u8)],
    w_values: &[u8],
    keep_w: bool,
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &(lo, hi) in pairs {
        for &w in w_values {
            for c in [false, true] {
                let x = (u16::from(hi) << 8) | u16::from(lo);
                let y = expect(x);
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LO, lo), (HI, hi), (STATUS_ADDR, status)],
                    allowed_changes: vec![LO, HI],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[LO] == (y & 0xFF) as u8
                            && sim.ram()[HI] == (y >> 8) as u8
                            && (!keep_w || sim.w() == w)
                    }),
                });
            }
        }
    }
    cases
}

fn cases32_over(
    expect: impl Fn(u32) -> u32 + Copy,
    words: &[[u8; 4]],
    w_values: &[u8],
    keep_w: bool,
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &bytes in words {
        for &w in w_values {
            for c in [false, true] {
                let x = u32::from_le_bytes(bytes);
                let y = expect(x).to_le_bytes();
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![
                        (LO, bytes[0]),
                        (HI, bytes[1]),
                        (B2, bytes[2]),
                        (B3, bytes[3]),
                        (STATUS_ADDR, status),
                    ],
                    allowed_changes: vec![LO, HI, B2, B3],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[LO] == y[0]
                            && sim.ram()[HI] == y[1]
                            && sim.ram()[B2] == y[2]
                            && sim.ram()[B3] == y[3]
                            && (!keep_w || sim.w() == w)
                    }),
                });
            }
        }
    }
    cases
}

/// Curated 16-bit edge pairs: zero, all-ones, the carry in/out of each
/// lane, and asymmetric patterns a lane-local bug would miss.
const EDGE_PAIRS: &[(u8, u8)] = &[
    (0x00, 0x00),
    (0xFF, 0xFF),
    (0xFF, 0x00),
    (0x00, 0xFF),
    (0xFF, 0x7F),
    (0x00, 0x80),
    (0x01, 0x00),
    (0xFE, 0xFF),
    (0x12, 0x34),
    (0xAB, 0xCD),
];

/// Every (lo, hi) pair at one representative `W` and clear entry carry:
/// the bit-pattern dimension, matching `shift_16bit.rs`' cost reasoning.
fn exhaustive16(expect: impl Fn(u16) -> u16 + Copy, keep_w: bool) -> Vec<Case> {
    let pairs: Vec<(u8, u8)> = (0..=255u16)
        .flat_map(|lo| (0..=255u16).map(move |hi| (lo as u8, hi as u8)))
        .collect();
    cases16_over(expect, &pairs, &[0x00], keep_w)
}

/// Both dimensions together: full patterns at one `W`, every `W` and both
/// entry carries over the edges.
fn verify_construction16(
    expect: impl Fn(u16) -> u16 + Copy,
    candidate: &Candidate,
    keep_w: bool,
) -> bool {
    verify(candidate, &exhaustive16(expect, keep_w))
        && verify(
            candidate,
            &cases16_over(expect, EDGE_PAIRS, W_SAMPLE, keep_w),
        )
}

fn lcg32(seed: u32, n: usize) -> Vec<[u8; 4]> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        out.push(state.to_le_bytes());
    }
    out
}

/// Structured 32-bit sample: each lane swept alone, carry/borrow edges at
/// every boundary, and a deterministic LCG for cross-lane interaction.
fn sample32() -> Vec<[u8; 4]> {
    let mut words = Vec::new();
    for lane in 0..4 {
        for v in [0x00u8, 0x01, 0x7F, 0x80, 0xFE, 0xFF] {
            let mut w = [0x00u8; 4];
            w[lane] = v;
            words.push(w);
        }
    }
    words.push([0xFF, 0xFF, 0xFF, 0xFF]);
    words.push([0x00, 0x00, 0x00, 0x00]);
    words.push([0xFF, 0xFF, 0x00, 0x00]);
    words.push([0x00, 0x00, 0xFF, 0xFF]);
    words.push([0xFF, 0x00, 0x00, 0x00]);
    words.push([0x00, 0xFF, 0xFF, 0xFF]);
    words.extend(lcg32(0x767, 128));
    words
}

fn verify_construction32(
    expect: impl Fn(u32) -> u32 + Copy,
    candidate: &Candidate,
    keep_w: bool,
) -> bool {
    verify(
        candidate,
        &cases32_over(expect, &sample32(), W_SAMPLE, keep_w),
    )
}

/// The plus-one alphabet: increment ops, flag tests, and the literal lane
/// forms the baseline uses. `W`-preservation is part of the case check,
/// so a `MOVLW` form can only win if nothing shorter verifies.
const PLUS1_ALPHABET: &[&str] = &[
    "incf 0x020,F,A",
    "incf 0x021,F,A",
    "btfsc 0xFD8,0,A",
    "btfss 0xFD8,0,A",
    "btfsc 0xFD8,2,A",
    "btfss 0xFD8,2,A",
    "movlw 0x01",
    "addwf 0x020,F,A",
    "addwf 0x020,W,A",
    "addwfc 0x021,F,A",
    "addwfc 0x021,W,A",
    "movwf 0x020,A",
    "movwf 0x021,A",
];

#[test]
fn shortest_inplace_add16_plus1() {
    let cases = cases16_over(|x| x.wrapping_add(1), EDGE_PAIRS, W_SAMPLE, true);
    let hits = shortest(PLUS1_ALPHABET, &cases, 4);
    assert!(
        !hits.is_empty(),
        "no verified 16-bit plus-one candidate up to length 4"
    );
    let len = hits[0].len();
    for hit in &hits {
        eprintln!("  {hit:?}");
    }
    assert!(
        len < 6,
        "expected to beat the 6-word staged-lane baseline, got {len}"
    );
}

const MINUS1_ALPHABET: &[&str] = &[
    "decf 0x020,F,A",
    "decf 0x021,F,A",
    "btfsc 0xFD8,0,A",
    "btfss 0xFD8,0,A",
    "btfsc 0xFD8,2,A",
    "btfss 0xFD8,2,A",
    "movlw 0x01",
    "subwf 0x020,F,A",
    "subwf 0x020,W,A",
    "subwfb 0x021,F,A",
    "subwfb 0x021,W,A",
    "movwf 0x020,A",
    "movwf 0x021,A",
];

#[test]
fn shortest_inplace_sub16_minus1() {
    let cases = cases16_over(|x| x.wrapping_sub(1), EDGE_PAIRS, W_SAMPLE, true);
    let hits = shortest(MINUS1_ALPHABET, &cases, 4);
    assert!(
        !hits.is_empty(),
        "no verified 16-bit minus-one candidate up to length 4"
    );
    let len = hits[0].len();
    for hit in &hits {
        eprintln!("  {hit:?}");
    }
    assert!(
        len < 6,
        "expected to beat the 6-word staged-lane baseline, got {len}"
    );
}

/// The landed plus-one chain (`VaArg` already emits the `INCF`-plus-skip
/// shape for pointer stepping): the low increment sets `C` exactly on
/// carry-out, the skip runs the high increment only then, and a skipped
/// high lane leaves `C` clear so longer chains compose.
fn inc16_candidate() -> Candidate {
    vec!["incf 0x020,F,A", "btfsc 0xFD8,0,A", "incf 0x021,F,A"]
}

#[test]
fn verify_inplace_add16_plus1() {
    let c = inc16_candidate();
    assert!(
        verify_construction16(|x| x.wrapping_add(1), &c, true),
        "INCF/BTFSC-C chain must add one over the full domain, W intact"
    );
    assert_eq!(
        c.len(),
        3,
        "chain must stay 3 words against the 6-word baseline"
    );
}

/// Mirror chain for minus one: `DECF` clears `C` exactly on borrow-out,
/// so `BTFSS C` runs the high decrement only then. (`Z` cannot serve
/// here: it reads result-zero, which is the no-borrow `0x01` case, not
/// the borrow-out `0x00` case.)
fn dec16_candidate() -> Candidate {
    vec!["decf 0x020,F,A", "btfss 0xFD8,0,A", "decf 0x021,F,A"]
}

#[test]
fn verify_inplace_sub16_minus1() {
    let c = dec16_candidate();
    assert!(
        verify_construction16(|x| x.wrapping_sub(1), &c, true),
        "DECF/BTFSS-C chain must subtract one over the full domain, W intact"
    );
    assert_eq!(
        c.len(),
        3,
        "chain must stay 3 words against the 6-word baseline"
    );
}

/// Generic in-place literal add: each lane stages its literal through `W`
/// but accumulates straight into the file register, dropping the store.
/// `MOVLW` never touches `C`, so the carry chain stays intact. Lane
/// literals ride as `&'static str` (what `Candidate` holds); each table
/// entry pairs `k` with its two preformatted low/high lines.
fn add16_lit_candidate(lo_line: &'static str, hi_line: &'static str) -> Candidate {
    vec![lo_line, "addwf 0x020,F,A", hi_line, "addwfc 0x021,F,A"]
}

const LITERALS16: &[(&str, &str, u16)] = &[
    ("movlw 0x02", "movlw 0x00", 0x0002),
    ("movlw 0x34", "movlw 0x12", 0x1234),
    ("movlw 0xFF", "movlw 0x00", 0x00FF),
    ("movlw 0x00", "movlw 0xFF", 0xFF00),
    ("movlw 0xFF", "movlw 0xFF", 0xFFFF),
    ("movlw 0x00", "movlw 0x80", 0x8000),
];

#[test]
fn verify_inplace_add16_literal() {
    for &(lo_line, hi_line, k) in LITERALS16 {
        let c = add16_lit_candidate(lo_line, hi_line);
        let ok = verify(
            &c,
            &cases16_over(move |x| x.wrapping_add(k), EDGE_PAIRS, &[0x00], false),
        );
        assert!(ok, "in-place add of 0x{k:04X} must verify over the edges");
    }
    let c = add16_lit_candidate("movlw 0x34", "movlw 0x12");
    assert!(
        verify_construction16(|x| x.wrapping_add(0x1234), &c, false),
        "in-place add of 0x1234 must verify over the full domain"
    );
    assert_eq!(c.len(), 4, "two read-modify-write lanes must stay 4 words");
}

/// Generic in-place literal subtract, same shape through `SUBWF`/`SUBWFB`.
fn sub16_lit_candidate(lo_line: &'static str, hi_line: &'static str) -> Candidate {
    vec![lo_line, "subwf 0x020,F,A", hi_line, "subwfb 0x021,F,A"]
}

#[test]
fn verify_inplace_sub16_literal() {
    for &(lo_line, hi_line, k) in LITERALS16 {
        let c = sub16_lit_candidate(lo_line, hi_line);
        let ok = verify(
            &c,
            &cases16_over(move |x| x.wrapping_sub(k), EDGE_PAIRS, &[0x00], false),
        );
        assert!(ok, "in-place sub of 0x{k:04X} must verify over the edges");
    }
    let c = sub16_lit_candidate("movlw 0x34", "movlw 0x12");
    assert!(
        verify_construction16(|x| x.wrapping_sub(0x1234), &c, false),
        "in-place sub of 0x1234 must verify over the full domain"
    );
    assert_eq!(c.len(), 4, "two read-modify-write lanes must stay 4 words");
}

/// Low-zero literal add (`k = 0x0100`): byte 0 adds nothing with no
/// carry-in, so the whole lane drops and the high lane takes the plain
/// form (its carry-in is provably zero, and the plain op reads none).
fn add16_lo0_candidate() -> Candidate {
    vec!["movlw 0x01", "addwf 0x021,F,A"]
}

#[test]
fn verify_inplace_add16_low_zero() {
    let c = add16_lo0_candidate();
    assert!(
        verify_construction16(|x| x.wrapping_add(0x0100), &c, false),
        "skipped low lane plus plain high lane must add 0x0100 over the full domain"
    );
    assert_eq!(
        c.len(),
        2,
        "one skipped lane plus one plain lane must stay 2 words"
    );
}

/// Low-zero literal subtract (`k = 0x0100`): subtracting nothing never
/// borrows, so the same skip applies through `SUBWF`.
fn sub16_lo0_candidate() -> Candidate {
    vec!["movlw 0x01", "subwf 0x021,F,A"]
}

#[test]
fn verify_inplace_sub16_low_zero() {
    let c = sub16_lo0_candidate();
    assert!(
        verify_construction16(|x| x.wrapping_sub(0x0100), &c, false),
        "skipped low lane plus plain high lane must subtract 0x0100 over the full domain"
    );
    assert_eq!(
        c.len(),
        2,
        "one skipped lane plus one plain lane must stay 2 words"
    );
}

/// 32-bit plus one: the 16-bit chain composed twice. A skipped middle
/// lane leaves `C` clear (its `INCF` never ran, and the low `INCF` set
/// `C` exactly on carry), so each link reads the previous link's carry.
fn inc32_candidate() -> Candidate {
    vec![
        "incf 0x020,F,A",
        "btfsc 0xFD8,0,A",
        "incf 0x021,F,A",
        "btfsc 0xFD8,0,A",
        "incf 0x022,F,A",
        "btfsc 0xFD8,0,A",
        "incf 0x023,F,A",
    ]
}

#[test]
fn verify_inplace_add32_plus1() {
    let c = inc32_candidate();
    assert!(
        verify_construction32(|x| x.wrapping_add(1), &c, true),
        "composed INCF chain must add one over the 32-bit sample, W intact"
    );
    assert_eq!(
        c.len(),
        7,
        "chain must stay 7 words against the 12-word baseline"
    );
}

fn dec32_candidate() -> Candidate {
    vec![
        "decf 0x020,F,A",
        "btfss 0xFD8,0,A",
        "decf 0x021,F,A",
        "btfss 0xFD8,0,A",
        "decf 0x022,F,A",
        "btfss 0xFD8,0,A",
        "decf 0x023,F,A",
    ]
}

#[test]
fn verify_inplace_sub32_minus1() {
    let c = dec32_candidate();
    assert!(
        verify_construction32(|x| x.wrapping_sub(1), &c, true),
        "composed DECF chain must subtract one over the 32-bit sample, W intact"
    );
    assert_eq!(
        c.len(),
        7,
        "chain must stay 7 words against the 12-word baseline"
    );
}

fn add32_lit_candidate(lines: &[&'static str; 4]) -> Candidate {
    vec![
        lines[0],
        "addwf 0x020,F,A",
        lines[1],
        "addwfc 0x021,F,A",
        lines[2],
        "addwfc 0x022,F,A",
        lines[3],
        "addwfc 0x023,F,A",
    ]
}

const LITERALS32: &[([&str; 4], u32)] = &[
    (
        ["movlw 0x02", "movlw 0x00", "movlw 0x00", "movlw 0x00"],
        0x00000002,
    ),
    (
        ["movlw 0x78", "movlw 0x56", "movlw 0x34", "movlw 0x12"],
        0x12345678,
    ),
    (
        ["movlw 0x00", "movlw 0x00", "movlw 0x01", "movlw 0x00"],
        0x00010000,
    ),
    (
        ["movlw 0xFF", "movlw 0xFF", "movlw 0xFF", "movlw 0xFF"],
        0xFFFFFFFF,
    ),
];

#[test]
fn verify_inplace_add32_literal() {
    for (lines, k) in LITERALS32 {
        let c = add32_lit_candidate(lines);
        let ok = verify(
            &c,
            &cases32_over(move |x| x.wrapping_add(*k), &sample32(), &[0x00], false),
        );
        assert!(ok, "in-place add of 0x{k:08X} must verify over the sample");
    }
    let c = add32_lit_candidate(&["movlw 0x78", "movlw 0x56", "movlw 0x34", "movlw 0x12"]);
    assert_eq!(c.len(), 8, "four read-modify-write lanes must stay 8 words");
}

fn sub32_lit_candidate(lines: &[&'static str; 4]) -> Candidate {
    vec![
        lines[0],
        "subwf 0x020,F,A",
        lines[1],
        "subwfb 0x021,F,A",
        lines[2],
        "subwfb 0x022,F,A",
        lines[3],
        "subwfb 0x023,F,A",
    ]
}

#[test]
fn verify_inplace_sub32_literal() {
    for (lines, k) in LITERALS32 {
        let c = sub32_lit_candidate(lines);
        let ok = verify(
            &c,
            &cases32_over(move |x| x.wrapping_sub(*k), &sample32(), &[0x00], false),
        );
        assert!(ok, "in-place sub of 0x{k:08X} must verify over the sample");
    }
    let c = sub32_lit_candidate(&["movlw 0x78", "movlw 0x56", "movlw 0x34", "movlw 0x12"]);
    assert_eq!(c.len(), 8, "four read-modify-write lanes must stay 8 words");
}
