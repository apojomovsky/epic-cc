//! Target 4 (epic-cc#526): 32-bit shifts by 4 (both directions), the i32
//! case the 16-bit
//! family "generalizes to on paper". The search bound is hopeless here
//! (`28^10` candidates, ~4 million hours; see the crate's timing probe in
//! `shift_16bit.rs`), so this constructs the natural in-place, W-only
//! generalization and verifies it, exactly as `shift_16bit.rs` did for the
//! amounts its own bound could not reach.
//!
//! The result is a verified candidate that **loses**: 21 words against the
//! 20-word unroll. That is worth recording rather than wiring, and it is
//! the answer to the ticket's "if a search finds nothing, record that
//! honestly". The arithmetic:
//!
//! - The 16-bit family's per-lane cost for a lane with a lower neighbour is
//!   `SWAPF x,W ; ANDLW 0x0F ; IORWF upper` after shifting `upper` itself,
//!   6 words, plus 3 words for the lowest lane (no neighbour).
//! - 4 lanes: 3 * 6 + 3 = 21 words.
//! - Unroll: `r` steps of `BCF` + one `RLCF` per live lane, `r * 5` = 20 at
//!   r == 4.
//!
//! So the construction grows 6 words per added lane while the unroll it
//! replaces grows `r` words per added lane; at 4 lanes the construction
//! loses at every nibble-boundary amount. This is the opposite of the
//! 16-bit case, where the construction grows 6 per lane and the baseline
//! only `2*r` per step.
//!
//! The 4-lane construction is verified over a curated 4-byte sample rather
//! than the full 2^32 domain: `2^32` cases at this crate's per-case cost is
//! days, and the ticket names the curated-sample treatment for exactly this
//! width.

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const BASE: usize = 0x020;
const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Curated 4-byte operands: all-zero, all-one, nibble-swapped lanes, a
/// distinct-nibble pattern in every position, and a couple of asymmetric
/// byte patterns. Full bit-pattern coverage is not the gate here (the
/// construction does not win); this is the check that the derivation is
/// sound, i.e. the "loses" verdict is on a correct candidate.
fn sample_words() -> Vec<[u8; 4]> {
    vec![
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        [0xF0, 0x0F, 0xF0, 0x0F],
        [0x0F, 0xF0, 0x0F, 0xF0],
        [0x12, 0x34, 0x56, 0x78],
        [0xAB, 0xCD, 0xEF, 0x01],
        [0x55, 0xAA, 0x55, 0xAA],
        [0x01, 0x80, 0x40, 0x20],
    ]
}

fn cases_with(
    shift: fn(u32, u32) -> u32,
    amount: u32,
    words: &[[u8; 4]],
    w_values: &[u8],
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &[b0, b1, b2, b3] in words {
        for &w in w_values {
            for c in [false, true] {
                let x = u32::from(b0)
                    | (u32::from(b1) << 8)
                    | (u32::from(b2) << 16)
                    | (u32::from(b3) << 24);
                let expect = shift(x, amount);
                let e = expect.to_le_bytes();
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![
                        (BASE, b0),
                        (BASE + 1, b1),
                        (BASE + 2, b2),
                        (BASE + 3, b3),
                        (STATUS_ADDR, status),
                    ],
                    allowed_changes: vec![BASE, BASE + 1, BASE + 2, BASE + 3],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[BASE] == e[0]
                            && sim.ram()[BASE + 1] == e[1]
                            && sim.ram()[BASE + 2] == e[2]
                            && sim.ram()[BASE + 3] == e[3]
                    }),
                });
            }
        }
    }
    cases
}

/// In-place, W-only, high-to-low (so no lane reads a byte already
/// overwritten): each lane above the lowest becomes `(lane << 4) | (lower
/// >> 4)`, the lowest just `<< 4`.
fn construction_shl4() -> Candidate {
    let mut c: Candidate = Vec::new();
    for i in (1..4usize).rev() {
        c.push(["swapf 0x021,F,A", "swapf 0x022,F,A", "swapf 0x023,F,A"][i - 1]);
        c.push("movlw 0xF0");
        c.push(["andwf 0x021,F,A", "andwf 0x022,F,A", "andwf 0x023,F,A"][i - 1]);
        c.push(["swapf 0x020,W,A", "swapf 0x021,W,A", "swapf 0x022,W,A"][i - 1]);
        c.push("andlw 0x0F");
        c.push(["iorwf 0x021,F,A", "iorwf 0x022,F,A", "iorwf 0x023,F,A"][i - 1]);
    }
    c.push("swapf 0x020,F,A");
    c.push("movlw 0xF0");
    c.push("andwf 0x020,F,A");
    c
}

#[test]
fn left_shift_32bit_by_4_construction_is_correct_and_loses() {
    let c = construction_shl4();
    assert_eq!(c.len(), 21, "the derivation is 3*6 + 3 words");
    assert!(
        verify(
            &c,
            &cases_with(u32::wrapping_shl, 4, &sample_words(), W_SAMPLE)
        ),
        "the 4-lane generalization must be a correct candidate for the \
         'loses' verdict to mean anything"
    );
    // The verdict this test exists to record: 21 words does NOT beat the
    // 20-word unroll (4 steps x (1 BCF + 4 RLCF)). If a future form ever
    // becomes shorter, wiring i32 left shifts becomes worthwhile and this
    // test should flip to asserting the win.
    assert!(c.len() >= 5 * 4, "21 words >= the 20-word unroll: it loses");
}

/// 32-bit right shift by 4, in-place, W-only, low-to-high (so no lane reads
/// a byte already overwritten): `dst[i] = (src[i]>>4) | ((src[i+1]&0x0F)<<4)`
/// for the three lower lanes, and the top lane just `>>4`. Same 21 words.
fn construction_shr4() -> Candidate {
    let mut c: Candidate = Vec::new();
    for i in 0..3usize {
        c.push(["swapf 0x020,F,A", "swapf 0x021,F,A", "swapf 0x022,F,A"][i]);
        c.push("movlw 0x0F");
        c.push(["andwf 0x020,F,A", "andwf 0x021,F,A", "andwf 0x022,F,A"][i]);
        c.push(["swapf 0x021,W,A", "swapf 0x022,W,A", "swapf 0x023,W,A"][i]);
        c.push("andlw 0xF0");
        c.push(["iorwf 0x020,F,A", "iorwf 0x021,F,A", "iorwf 0x022,F,A"][i]);
    }
    c.push("swapf 0x023,W,A");
    c.push("andlw 0x0F");
    c.push("movwf 0x023,A");
    c
}

#[test]
fn right_shift_32bit_by_4_construction_is_correct_and_loses() {
    let c = construction_shr4();
    assert_eq!(c.len(), 21, "the derivation is 3*6 + 3 words");
    assert!(
        verify(
            &c,
            &cases_with(u32::wrapping_shr, 4, &sample_words(), W_SAMPLE)
        ),
        "the 4-lane right generalization must be a correct candidate"
    );
    assert!(c.len() >= 5 * 4, "21 words >= the 20-word unroll: it loses");
}
