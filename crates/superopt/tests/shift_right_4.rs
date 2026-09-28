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

use superopt::specs::{
    shift_right_4_candidate as construction_r4, shift_right_4_cases_over as cases_over,
    shift_right_4_exhaustive as cases_exhaustive,
};
use superopt::verify;

#[test]
fn right_shift_4_single_lane_is_exact_over_the_byte_domain() {
    let c = construction_r4();
    assert!(
        verify(&c, &cases_exhaustive()),
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
