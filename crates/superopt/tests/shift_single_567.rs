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

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LANE: usize = 0x020;
const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

fn cases_over(shift: fn(u8, u32) -> u8, amount: u32, pairs: &[u8], w_values: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    for &lane in pairs {
        for &w in w_values {
            for c in [false, true] {
                let expect = shift(lane, amount);
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

fn all_bytes() -> Vec<u8> {
    (0..=u8::MAX).collect()
}

fn construction_shl(r: u32) -> Candidate {
    let mut c: Candidate = vec!["swapf 0x020,W,A", "andlw 0xF0", "movwf 0x020,A"];
    for _ in 4..r {
        c.push("bcf 0xFD8,0,A");
        c.push("rlcf 0x020,F,A");
    }
    c
}

fn construction_shr(r: u32) -> Candidate {
    let mut c: Candidate = vec!["swapf 0x020,W,A", "andlw 0x0F", "movwf 0x020,A"];
    for _ in 4..r {
        c.push("bcf 0xFD8,0,A");
        c.push("rrcf 0x020,F,A");
    }
    c
}

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
