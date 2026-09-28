//! epic-cc#573: single-lane residuals at amounts 5-7, the shift-chain the
//! density profiler still attributes on the menu demo. Each switch arm of
//! `EPIC_IRQ_GetFlag` extracts a different bit field (`>>1` through `>>6`
//! on one byte); the `>>4` arms already use the landed nibble form, the
//! `>>1`/`>>2`/`>>3` arms are settled or excluded, and the `>>5`/`>>6`
//! arms (plus one `>>7` and one `<<6` elsewhere) still unroll: 10/12/14
//! words of `BCF` + `RRCF`/`RLCF` per site.
//!
//! The construction is the landed nibble form plus one carry-seeded
//! rotate per remaining bit: 3 + 2*(r-4) words against the unroll's 2*r.
//! Each extra rotate needs its own `BCF`: a single lane has no lower lane
//! to take carry from, so sharing one seed would shift the previous
//! step's bit 7 into bit 0. Like the landed forms it clobbers `W`, so the
//! same dead-`W` precondition applies.

use superopt::specs::{
    single_all_bytes as all_bytes, single_cases_over as cases_over,
    single_construction_shl as construction_shl, single_construction_shr as construction_shr,
};
use superopt::verify;

const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

#[test]
fn single_lane_left_residuals_are_exact_over_the_byte_domain() {
    for r in [5, 6, 7] {
        assert!(
            verify(
                &construction_shl(r),
                &cases_over(u8::wrapping_shl, r, &all_bytes(), &[0x00])
            ),
            "shl by {r} must be exact over all 256 inputs"
        );
    }
}

#[test]
fn single_lane_left_residuals_are_independent_of_w() {
    for r in [5, 6, 7] {
        assert!(
            verify(
                &construction_shl(r),
                &cases_over(u8::wrapping_shl, r, &all_bytes(), W_SAMPLE)
            ),
            "shl by {r} must hold for every entry W"
        );
    }
}

#[test]
fn single_lane_left_residuals_beat_the_unroll() {
    for r in [5, 6, 7] {
        let c = construction_shl(r);
        assert_eq!(
            c.len(),
            3 + 2 * (r as usize - 4),
            "nibble plus one BCF+RLCF per extra bit"
        );
        assert!(
            c.len() < 2 * r as usize,
            "shl by {r}: {} beats the {}-word unroll",
            c.len(),
            2 * r
        );
    }
}

#[test]
fn single_lane_right_residuals_are_exact_over_the_byte_domain() {
    for r in [5, 6, 7] {
        assert!(
            verify(
                &construction_shr(r),
                &cases_over(u8::wrapping_shr, r, &all_bytes(), &[0x00])
            ),
            "lshr by {r} must be exact over all 256 inputs"
        );
    }
}

#[test]
fn single_lane_right_residuals_are_independent_of_w() {
    for r in [5, 6, 7] {
        assert!(
            verify(
                &construction_shr(r),
                &cases_over(u8::wrapping_shr, r, &all_bytes(), W_SAMPLE)
            ),
            "lshr by {r} must hold for every entry W"
        );
    }
}

#[test]
fn single_lane_right_residuals_beat_the_unroll() {
    for r in [5, 6, 7] {
        let c = construction_shr(r);
        assert_eq!(
            c.len(),
            3 + 2 * (r as usize - 4),
            "nibble plus one BCF+RRCF per extra bit"
        );
        assert!(
            c.len() < 2 * r as usize,
            "lshr by {r}: {} beats the {}-word unroll",
            c.len(),
            2 * r
        );
    }
}
