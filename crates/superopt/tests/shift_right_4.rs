//! Target 3 (epic-cc#526): constant-count RIGHT shifts, which still unroll
//! per bit. `isel-pic18` lowers a constant right shift of amount `r` over
//! `active` lanes as `r` steps of `BCF STATUS,C` + one `RRCF` per lane,
//! 3 words/step, i.e. `3*r` words for a single lane.
//!
//! The left-shift work (docs/41) landed a single-lane 4-bit form as
//! `SWAPF lane,W ; ANDLW 0xF0 ; MOVWF lane`, 3 words, replacing the 12-word
//! unroll at r == 4. The mirror claim for right shifts is
//! `SWAPF lane,W ; ANDLW 0x0F ; MOVWF lane`: a right shift by 4 moves the
//! high nibble down, so the surviving bits are the source's high nibble
//! placed in the low position, which is what `SWAPF` then `ANDLW 0x0F`
//! produces.
//!
//! The ticket's own history is the reason this is verified rather than
//! asserted: docs/41 records an unsound `CLRF`-first candidate caught only
//! by review (epic-cc#501). So the construction is checked before any
//! `isel-pic18` change, over the full 8-bit input domain.

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LANE: usize = 0x020;

/// Cases for a single-lane right shift by `amount`, over `pairs` x `w_values`
/// x both `C` values. Mirrors `shift_16bit.rs`'s `cases_over` for the
/// opposite direction, with the lane's own byte as the only payload.
fn cases_over(amount: u32, pairs: &[u8], w_values: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    for &lane in pairs {
        for &w in w_values {
            for c in [false, true] {
                let expect = lane.wrapping_shr(amount);
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LANE, lane), (STATUS_ADDR, status)],
                    allowed_changes: vec![LANE],
                    check: Box::new(move |sim: &Pic18| sim.ram()[LANE] == expect),
                });
            }
        }
    }
    cases
}

/// Every byte value, crossed with a single `W`/`C`: the bit-pattern
/// dimension, which is what a nibble trick can get wrong.
fn cases_exhaustive(amount: u32) -> Vec<Case> {
    let all: Vec<u8> = (0..=255u8).collect();
    cases_over(amount, &all, &[0x00])
}

/// The 4-bit single-lane right shift. Three words against the unroll's 12,
/// and it clobbers `W` (like the landed left-shift forms).
fn construction_r4() -> Candidate {
    vec!["swapf 0x020,W,A", "andlw 0x0F", "movwf 0x020,A"]
}

#[test]
fn right_shift_4_single_lane_is_exact_over_the_byte_domain() {
    let c = construction_r4();
    assert!(
        verify(&c, &cases_exhaustive(4)),
        "r==4 construction must be exact over all 256 inputs"
    );
}

#[test]
fn right_shift_4_single_lane_is_independent_of_w() {
    let candidate = construction_r4();
    // Every byte at a curated W sample: catches a construction that depends
    // on stale entry state (the #501 failure class). `cases_over` already
    // crosses both `C` values.
    let all: Vec<u8> = (0..=255u8).collect();
    for w in [0x00u8, 0xFF, 0x2A] {
        assert!(
            verify(&candidate, &cases_over(4, &all, &[w])),
            "r==4 must hold for entry W={w:#04x}"
        );
    }
}

#[test]
fn right_shift_4_beats_the_unroll_it_would_replace() {
    // The comparison the wiring decision rests on: construction words vs
    // the 3*r unroll.
    let unroll = 3 * 4;
    assert!(
        construction_r4().len() < unroll,
        "construction ({}) must beat the unroll ({unroll})",
        construction_r4().len()
    );
}
