//! Target 2 generalized to 16 bits (epic-cc#520, follow-up to epic-cc#514):
//! epic-cc#505's actual repro is `unsigned int x; x <<= 4;`, 12 words
//! (four steps of `BCF STATUS,C` + `RLCF`/`RLCF`). `crates/superopt/tests/
//! shift_left_4.rs` only searched the 8-bit, shift-by-4-in-place case; this
//! file covers the 16-bit contract and shift amounts 1-7 (8 is already
//! #470's byte-move case, and amounts above 7 are equivalent to amounts
//! 0-7 combined with whole-byte moves, out of scope here).
//!
//! `isel-pic18`'s current lowering for any shift amount N is N steps of
//! `BCF STATUS,C` + `RLCF lo,F` + `RLCF hi,F`, 3 words/step, so the
//! baseline to beat is **3*N words** for every amount in this file.
//!
//! The alphabet a 16-bit, two-register contract needs (rotate ops plus
//! the nibble-swap family) is 28 symbols; `28^9` (amount 4's baseline
//! length) is past what a bounded-effort exhaustive search can enumerate.
//! Two independent result sources per amount:
//!
//! - **Bounded exhaustive search**, up to length 3, over the combined
//!   rotate+nibble alphabet, checked against a curated case sample (same
//!   shape as `shift_left_4.rs`'s). The alphabet's own growth (added to
//!   express the amount 6/7 constructions below) pushed a length-4 bound
//!   for this alphabet size well past what stays fast in a debug build;
//!   3 still covers amount 1's 3-word baseline, so a floor found there is
//!   a real answer, while every other amount finds nothing this short
//!   within the bound, a property of the bound, not a claim the baseline
//!   is optimal.
//! - **A constructed candidate**, one per amount 4-7, each checked with
//!   `verify()` against the full 65536-input domain at one representative
//!   `W` (`cases_exhaustive`) plus the curated sample across every `W` in
//!   `W_SAMPLE` (`verify_construction` combines both): a construction is
//!   a single fixed candidate, so this stays fast even though the search
//!   above does not. Amount 4: the
//!   nibble-swap generalization from the 8-bit target, `SWAPF` both
//!   bytes, mask, recombine the nibble that straddles the byte boundary.
//!   This is one instance of a general family (rotate each byte left by
//!   n, mask, recombine the straddling bits), whose cost is
//!   `2 * rotate_cost(n) + 7`; `SWAPF` makes `rotate_cost(4) == 1`
//!   (9 words), and `RRNCF` (rotate right, no carry) makes
//!   `rotate_cost(6) == 2` (11 words, amount 6's construction here).
//!   Amount 5 composes the amount-4 construction with one more standard
//!   rotate step (12 words; the family's own amount-5 instance is 13,
//!   worse). Amount 7 uses a different, shorter trick: a 16-bit logical
//!   right-shift-by-1 (through `C`, seeded 0) followed by a byte move
//!   turns `x << 7` into 7 words, beating even a `rotate_cost(7) == 1`
//!   family instance's 9.
//!
//! **Every construction here clobbers `W`**, unlike the baseline (`BCF`+
//! `RLCF`+`RLCF` never touches `W`). `run_case`'s clobber check does not
//! catch this by design (`W` is excepted, same as `STATUS`; see
//! `crates/superopt/src/lib.rs`), so it is stated here instead: whoever
//! lands one of these in `isel-pic18` needs to confirm `W` is dead at
//! every call site first.
//!
//! Destination/source registers: `LO` = 0x020, `HI` = 0x021 (in place,
//! matching how `x <<= n` compiles: same two addresses in and out,
//! little-endian, `LO` at the lower address).

use pic14_sim::Pic18;
use superopt::{shortest, verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LO: usize = 0x020;
const HI: usize = 0x021;

/// One case per (lo, hi, w, c) tuple built from `pairs`, `w_values`, both
/// `C` values. Shared by the curated search sample and the exhaustive
/// construction check below; only the `pairs` domain differs between them.
fn cases_over(amount: u32, pairs: &[(u8, u8)], w_values: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    for &(lo, hi) in pairs {
        for &w in w_values {
            for c in [false, true] {
                let x = (u16::from(hi) << 8) | u16::from(lo);
                let expect = x.wrapping_shl(amount);
                let expect_lo = (expect & 0xFF) as u8;
                let expect_hi = (expect >> 8) as u8;
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LO, lo), (HI, hi), (STATUS_ADDR, status)],
                    allowed_changes: vec![LO, HI],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[LO] == expect_lo && sim.ram()[HI] == expect_hi
                    }),
                });
            }
        }
    }
    cases
}

/// A curated, non-exhaustive set of (lo, hi) pairs: all-zero, all-one, a
/// nibble-boundary pair in each direction, a couple of asymmetric byte
/// patterns, and a case with distinct nibbles in every position. Used only
/// for `search`, where case count multiplies the already-large candidate
/// count; `verify_construction` below uses the full domain instead.
fn cases(amount: u32, w_values: &[u8]) -> Vec<Case> {
    const PAIRS: &[(u8, u8)] = &[
        (0x00, 0x00),
        (0xFF, 0xFF),
        (0x0F, 0xF0),
        (0xF0, 0x0F),
        (0x01, 0x80),
        (0x12, 0x34),
        (0xAB, 0xCD),
        (0x55, 0xAA),
    ];
    cases_over(amount, PAIRS, w_values)
}

/// Every (lo, hi) pair, all 65536 of them, crossed with `w_values`. Full
/// bit-pattern coverage is what actually exercises the nibble/byte-
/// boundary arithmetic these constructions do; `w_values` stays a single
/// value here to keep the case count (and so the wall-clock cost of this
/// crate's per-case allocation) reasonable in a debug build (measured:
/// the full `w_values.len() == 3, both C` cross costs 2+ minutes across
/// four constructions, too slow for the default suite; a single `W`,
/// single `C` cuts that to seconds). `W`/`C` independence is checked
/// separately, over the curated pairs, by `verify_construction`.
fn cases_exhaustive(amount: u32, w_values: &[u8]) -> Vec<Case> {
    let pairs: Vec<(u8, u8)> = (0..=255u16)
        .flat_map(|lo| (0..=255u16).map(move |hi| (lo as u8, hi as u8)))
        .collect();
    cases_over(amount, &pairs, w_values)
}

/// Two independent, complementary checks, both required: every bit
/// pattern at a single representative `W` (the dimension that actually
/// exercises the nibble/byte-boundary arithmetic), and every `W` in
/// `W_SAMPLE` at a curated bit-pattern sample (the dimension that would
/// catch a construction accidentally depending on stale entry state, the
/// exact class of bug epic-cc#514's review found in this crate once).
fn verify_construction(amount: u32, candidate: &Candidate) -> bool {
    verify(candidate, &cases_exhaustive(amount, &[0x00]))
        && verify(candidate, &cases(amount, W_SAMPLE))
}

/// Combined rotate + nibble-swap alphabet, curated rather than the full
/// ISA for the same reason `shift_left_4.rs`'s alphabet is curated: masks
/// and rotates for a byte-boundary shift are naturally shaped like this,
/// and the full opcode/literal space is a much larger spike. Includes
/// both rotate directions (`RLCF`/`RRCF`/`RRNCF`): epic-cc#520's review
/// found the first draft's left-only alphabet could not express the
/// amount 6/7 constructions below at any search length, which is why
/// they are hand-constructed rather than found by `search`.
const ALPHABET: &[&str] = &[
    "bcf 0xFD8,0,A",
    "rlcf 0x020,F,A",
    "rlcf 0x021,F,A",
    "rrcf 0x020,F,A",
    "rrcf 0x021,F,A",
    "rrncf 0x020,F,A",
    "rrncf 0x021,F,A",
    "swapf 0x020,F,A",
    "swapf 0x020,W,A",
    "swapf 0x021,F,A",
    "swapf 0x021,W,A",
    "andlw 0xF0",
    "andlw 0x0F",
    "andlw 0xC0",
    "andlw 0x3F",
    "iorlw 0xF0",
    "iorlw 0x0F",
    "andwf 0x020,F,A",
    "andwf 0x021,F,A",
    "iorwf 0x020,F,A",
    "iorwf 0x021,F,A",
    "clrf 0x020,A",
    "movwf 0x020,A",
    "movwf 0x021,A",
    "movf 0x020,W,A",
    "movf 0x021,W,A",
    "movlw 0xF0",
    "movlw 0x0F",
];

const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Bounded exhaustive search (up to length 3) for `amount`, reporting
/// whatever it finds (or does not) against the baseline, honestly: no
/// verified hit within the bound means the bound was not enough, not
/// that no shorter sequence exists.
fn search(amount: u32, max_len: usize) -> Vec<Candidate> {
    let cases = cases(amount, W_SAMPLE);
    shortest(ALPHABET, &cases, max_len)
}

/// Amount 4's construction: `SWAPF` both bytes, mask, recombine the
/// straddling nibble. `rotate_cost(4) == 1` (`SWAPF` is a single
/// instruction), so the general family's `2 * rotate_cost(n) + 7` gives
/// 9 words.
fn construction_amount_4() -> Candidate {
    vec![
        "swapf 0x021,F,A",
        "movlw 0xF0",
        "andwf 0x021,F,A",
        "swapf 0x020,W,A",
        "andlw 0x0F",
        "iorwf 0x021,F,A",
        "swapf 0x020,F,A",
        "movlw 0xF0",
        "andwf 0x020,F,A",
    ]
}

/// Amount 5: the amount-4 construction plus one more standard rotate step
/// (12 words). Valid because a left shift by 4 then 1 never needs a bit
/// the first step already discarded. The family's own amount-5 instance
/// (`rotate_cost(5) == 3`, three `RLCF`s or `RRNCF`+`RRCF`) costs 13,
/// worse than composing, so this is the better of the two, not a
/// shortcut taken for lack of trying the family.
fn construction_amount_5() -> Candidate {
    let mut c = construction_amount_4();
    c.push("bcf 0xFD8,0,A");
    c.push("rlcf 0x020,F,A");
    c.push("rlcf 0x021,F,A");
    c
}

/// Amount 6: the general family's own instance, `rotate_cost(6) == 2`
/// (`RRNCF` applied twice per byte is a rotate-right-2, equal to
/// rotate-left-6), giving `2*2 + 7 = 11` words. Masks are `0xFF << 6 =
/// 0xC0` (keep the top 2 bits after rotation) and `(1 << 6) - 1 = 0x3F`
/// (the 6 bits that straddle into the next byte).
fn construction_amount_6() -> Candidate {
    vec![
        "rrncf 0x021,F,A",
        "rrncf 0x021,F,A",
        "movlw 0xC0",
        "andwf 0x021,F,A",
        "rrncf 0x020,F,A",
        "rrncf 0x020,F,A",
        "movf 0x020,W,A",
        "andlw 0x3F",
        "iorwf 0x021,F,A",
        "movlw 0xC0",
        "andwf 0x020,F,A",
    ]
}

/// Amount 7: not the family (`rotate_cost(7) == 1` via one `RRNCF` would
/// give 9 words), but a shorter trick specific to being one bit short of
/// a byte boundary. `x << 7` mod 65536 keeps only `x`'s low 9 bits,
/// shifted up by 7; a 16-bit logical right-shift-by-1 (`BCF C` then
/// `RRCF hi,F` then `RRCF lo,F`, carry seeded 0 so no bit wraps back in)
/// produces exactly `x >> 1` across the pair, and the byte that lands in
/// `LO` after that shift is precisely the byte `HI` needs post-shift-left-7,
/// with `LO` itself then cleared and rotated once more to place the one
/// remaining bit. 7 words total.
fn construction_amount_7() -> Candidate {
    vec![
        "bcf 0xFD8,0,A",
        "rrcf 0x021,F,A",
        "rrcf 0x020,F,A",
        "movf 0x020,W,A",
        "movwf 0x021,A",
        "clrf 0x020,A",
        "rrcf 0x020,F,A",
    ]
}

fn report(amount: u32, search_hits: &[Candidate], constructed: Option<&Candidate>) {
    let baseline = 3 * amount as usize;
    eprintln!("--- shift-left-{amount} (16-bit), baseline {baseline} words ---");
    if search_hits.is_empty() {
        eprintln!("  bounded search (<=3 words): nothing found within bound");
    } else {
        eprintln!(
            "  bounded search: floor {} words, {} candidate(s)",
            search_hits[0].len(),
            search_hits.len()
        );
        for hit in search_hits {
            eprintln!("    {hit:?}");
        }
    }
    if let Some(c) = constructed {
        eprintln!(
            "  constructed candidate: {} words, {} fewer than baseline",
            c.len(),
            baseline as isize - c.len() as isize
        );
        eprintln!("    {c:?}");
    }
}

#[test]
fn shift_left_1() {
    let hits = search(1, 3);
    report(1, &hits, None);
    assert!(
        !hits.is_empty(),
        "expected the 3-word baseline itself to verify within the search bound"
    );
    assert_eq!(
        hits[0].len(),
        3,
        "shift-by-1's baseline is already minimal; a shorter or (with a narrower alphabet) \
         unconfirmed result here would silently change what this test claims"
    );
}

#[test]
fn shift_left_2() {
    let hits = search(2, 3);
    report(2, &hits, None);
    // A hit here would be real news (nothing this short was known for
    // amount 2), so it fails loudly instead of passing silently: the
    // next step would be to raise the search bound and confirm, not to
    // let this test's green stay quiet about it.
    assert!(
        hits.is_empty(),
        "found an unexpected hit for shift-by-2 within the search bound, investigate: {hits:?}"
    );
}

#[test]
fn shift_left_3() {
    let hits = search(3, 3);
    report(3, &hits, None);
    assert!(
        hits.is_empty(),
        "found an unexpected hit for shift-by-3 within the search bound, investigate: {hits:?}"
    );
}

#[test]
fn shift_left_4() {
    let hits = search(4, 3);
    let constructed = construction_amount_4();
    assert!(
        verify_construction(4, &constructed),
        "amount-4 construction failed exhaustive-input verification"
    );
    report(4, &hits, Some(&constructed));
    assert_eq!(constructed.len(), 9);
}

#[test]
fn shift_left_5() {
    let hits = search(5, 3);
    let constructed = construction_amount_5();
    assert!(
        verify_construction(5, &constructed),
        "amount-5 construction failed exhaustive-input verification"
    );
    report(5, &hits, Some(&constructed));
    assert_eq!(constructed.len(), 12);
}

#[test]
fn shift_left_6() {
    let hits = search(6, 3);
    let constructed = construction_amount_6();
    assert!(
        verify_construction(6, &constructed),
        "amount-6 construction failed exhaustive-input verification"
    );
    report(6, &hits, Some(&constructed));
    assert_eq!(constructed.len(), 11);
}

#[test]
fn shift_left_7() {
    let hits = search(7, 3);
    let constructed = construction_amount_7();
    assert!(
        verify_construction(7, &constructed),
        "amount-7 construction failed exhaustive-input verification"
    );
    report(7, &hits, Some(&constructed));
    assert_eq!(constructed.len(), 7);
}

/// epic-cc#528: the deep run docs/41's table left unanswered. Lengths 4 and
/// 5 over the 28-symbol alphabet, curated case sample, timing recorded.
/// Length 3 is the previous bound and already covered by the unit tests
/// above, so this starts at 4.
#[test]
#[ignore = "deep run (minutes): epic-cc#528, run with --ignored"]
fn deep_search_amounts_2_and_3() {
    for amount in [2u32, 3] {
        let baseline = 3 * amount as usize;
        let t0 = std::time::Instant::now();
        let hits = search(amount, 5);
        let el = t0.elapsed();
        eprintln!(
            "DEEP amount={amount} baseline={baseline} words: {} hit(s) in {el:?}",
            hits.len()
        );
        for h in hits.iter().take(5) {
            eprintln!("   {} words: {}", h.len(), h.join(" ; "));
        }
    }
}
