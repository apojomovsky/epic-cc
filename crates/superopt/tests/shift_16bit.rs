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
//! the nibble-swap family) is ~21 symbols; `21^9` (amount 4's baseline
//! length) is past what a bounded-effort exhaustive search can enumerate.
//! Two independent result sources per amount, both verified through
//! `verify()`/`shortest()` against the same case set, neither hand-traced:
//!
//! - **Bounded exhaustive search**, up to length 4, over the combined
//!   rotate+nibble alphabet. Amount 1's 3-word baseline is inside this
//!   bound, so a floor found there is a real answer; larger amounts
//!   mostly find nothing this short within the bound, a property of the
//!   bound, not a claim the baseline is optimal.
//! - **A constructed candidate.** Amount 4: the nibble-swap generalization
//!   from the 8-bit target, `SWAPF` both bytes, mask, recombine the
//!   nibble that straddles the byte boundary. Amounts 5-7: that same
//!   amount-4 construction with (amount - 4) more standard rotate steps
//!   appended, valid because a left shift by 4 then k never needs a bit
//!   the first step already discarded.
//!
//! Destination/source registers: `LO` = 0x020, `HI` = 0x021 (in place,
//! matching how `x <<= n` compiles: same two addresses in and out,
//! little-endian, `LO` at the lower address).

use pic14_sim::Pic18;
use superopt::{shortest, verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LO: usize = 0x020;
const HI: usize = 0x021;

/// A curated, non-exhaustive set of (lo, hi) pairs: all-zero, all-one, a
/// nibble-boundary pair in each direction, a couple of asymmetric byte
/// patterns, and a case with distinct nibbles in every position. Crossed
/// with entry `W` and entry `C`, both dimensions a correct candidate must
/// not depend on (same reasoning as `shift_left_4.rs`, doubled here since
/// the alphabet spans two registers).
fn cases(amount: u32, w_values: &[u8]) -> Vec<Case> {
    let pairs: &[(u8, u8)] = &[
        (0x00, 0x00),
        (0xFF, 0xFF),
        (0x0F, 0xF0),
        (0xF0, 0x0F),
        (0x01, 0x80),
        (0x12, 0x34),
        (0xAB, 0xCD),
        (0x55, 0xAA),
    ];
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

/// Combined rotate + nibble-swap alphabet, curated rather than the full
/// ISA for the same reason `shift_left_4.rs`'s alphabet is curated: masks
/// and rotates for a byte-boundary shift are naturally shaped like this,
/// and the full opcode/literal space is a much larger spike.
const ALPHABET: &[&str] = &[
    "bcf 0xFD8,0,A",
    "rlcf 0x020,F,A",
    "rlcf 0x021,F,A",
    "swapf 0x020,F,A",
    "swapf 0x020,W,A",
    "swapf 0x021,F,A",
    "swapf 0x021,W,A",
    "andlw 0xF0",
    "andlw 0x0F",
    "iorlw 0xF0",
    "iorlw 0x0F",
    "andwf 0x020,F,A",
    "andwf 0x021,F,A",
    "iorwf 0x020,F,A",
    "iorwf 0x021,F,A",
    "movwf 0x020,A",
    "movwf 0x021,A",
    "movf 0x020,W,A",
    "movf 0x021,W,A",
    "movlw 0xF0",
    "movlw 0x0F",
];

const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Bounded exhaustive search (up to length 4) for `amount`, reporting
/// whatever it finds (or does not) against the baseline, honestly: no
/// verified hit within the bound means the bound was not enough, not
/// that no shorter sequence exists.
fn search(amount: u32, max_len: usize) -> Vec<Candidate> {
    let cases = cases(amount, W_SAMPLE);
    shortest(ALPHABET, &cases, max_len)
}

/// Amount 4's construction: `SWAPF` both bytes, mask, recombine the
/// straddling nibble. Verified directly (not found by `search`, which
/// cannot reach 9 words at this alphabet size in a bounded run).
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

/// Amount 4's construction, then `extra` more standard rotate steps
/// (`BCF`+`RLCF`+`RLCF`), for amounts 5-7. Composing is valid here
/// specifically because a left shift never needs a bit this construction
/// already discarded: shift-by-(4+k) really is "shift left 4, then shift
/// left k" with no information the second step needs and the first threw
/// away.
fn construction_amount_4_plus(extra: u32) -> Candidate {
    let mut c = construction_amount_4();
    for _ in 0..extra {
        c.push("bcf 0xFD8,0,A");
        c.push("rlcf 0x020,F,A");
        c.push("rlcf 0x021,F,A");
    }
    c
}

fn report(amount: u32, search_hits: &[Candidate], constructed: Option<&Candidate>) {
    let baseline = 3 * amount as usize;
    eprintln!("--- shift-left-{amount} (16-bit), baseline {baseline} words ---");
    if search_hits.is_empty() {
        eprintln!("  bounded search (<=4 words): nothing found within bound");
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
            "  constructed candidate: {} words (baseline - {})",
            c.len(),
            baseline as isize - c.len() as isize
        );
        eprintln!("    {c:?}");
    }
}

#[test]
fn shift_left_1() {
    let hits = search(1, 4);
    report(1, &hits, None);
    // Baseline is already 3 words (BCF+RLCF+RLCF), which is inside the
    // search bound: a floor found here that is not shorter than 3 is a
    // real confirmation the baseline is already minimal for this amount,
    // not an artifact of an insufficient bound.
    if !hits.is_empty() {
        assert!(
            hits[0].len() >= 3,
            "found something shorter than the 3-word baseline for shift-by-1, unexpected: {:?}",
            hits[0]
        );
    }
}

#[test]
fn shift_left_2() {
    let hits = search(2, 4);
    report(2, &hits, None);
}

#[test]
fn shift_left_3() {
    let hits = search(3, 4);
    report(3, &hits, None);
}

#[test]
fn shift_left_4() {
    let hits = search(4, 4);
    let constructed = construction_amount_4();
    let case_set = cases(4, W_SAMPLE);
    assert!(
        verify(&constructed, &case_set),
        "hand-derived amount-4 construction failed to verify"
    );
    report(4, &hits, Some(&constructed));
    assert!(
        constructed.len() < 12,
        "expected the constructed candidate to beat the 12-word baseline, got {}",
        constructed.len()
    );
}

#[test]
fn shift_left_5() {
    let hits = search(5, 4);
    let constructed = construction_amount_4_plus(1);
    let case_set = cases(5, W_SAMPLE);
    assert!(
        verify(&constructed, &case_set),
        "constructed amount-5 candidate failed to verify"
    );
    report(5, &hits, Some(&constructed));
    assert!(constructed.len() < 15, "got {}", constructed.len());
}

#[test]
fn shift_left_6() {
    let hits = search(6, 4);
    let constructed = construction_amount_4_plus(2);
    let case_set = cases(6, W_SAMPLE);
    assert!(
        verify(&constructed, &case_set),
        "constructed amount-6 candidate failed to verify"
    );
    report(6, &hits, Some(&constructed));
    assert!(constructed.len() < 18, "got {}", constructed.len());
}

#[test]
fn shift_left_7() {
    let hits = search(7, 4);
    let constructed = construction_amount_4_plus(3);
    let case_set = cases(7, W_SAMPLE);
    assert!(
        verify(&constructed, &case_set),
        "constructed amount-7 candidate failed to verify"
    );
    report(7, &hits, Some(&constructed));
    assert!(constructed.len() < 21, "got {}", constructed.len());
}
