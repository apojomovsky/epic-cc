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
//! Extending to amount 5 costs the nibble form plus one more single-bit
//! pass, 5 words, so the construction is `5r + 1` against the unroll's
//! `5r` and loses by exactly one word at every amount (both are built and
//! verified here). This is the opposite of the 16-bit case, where the
//! construction grows 6 per lane and the baseline only `2*r` per step, so
//! the 16-bit forms win at every amount while these never do.
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

/// The straightforward amount-5 extension: the amount-4 nibble form followed
/// by one more single-bit pass (`BCF` + one rotate per lane, 5 words). This is
/// what makes the cost model explicit: the construction does not stay 21
/// words, it grows 5 per additional amount (21 + 5*(r-4) = 5r + 1) while the
/// unroll is 5r, so it loses by exactly one word at *every* amount, not just
/// amount 4. A cleverer per-amount form (the 16-bit target found one for
/// 6 and 7) was not searched here.
fn construction_shl5() -> Candidate {
    let mut c = construction_shl4();
    c.push("bcf 0xFD8,0,A");
    for a in [
        "rlcf 0x020,F,A",
        "rlcf 0x021,F,A",
        "rlcf 0x022,F,A",
        "rlcf 0x023,F,A",
    ] {
        c.push(a);
    }
    c
}

#[test]
fn left_shift_32bit_by_5_is_correct_and_also_loses() {
    let c = construction_shl5();
    assert_eq!(c.len(), 21 + 5, "amount 4 form plus one bit pass");
    assert!(
        verify(
            &c,
            &cases_with(u32::wrapping_shl, 5, &sample_words(), W_SAMPLE)
        ),
        "the amount-5 extension must be a correct candidate"
    );
    assert!(
        c.len() > 5 * 5,
        "21 + 5 = 26 words against the 25-word unroll: the cost model is 5r+1 vs 5r"
    );
}

// epic-cc#549: fused 32-bit left shifts at amounts 6 and 7. The ticket's
// amount-7 byte-move form is unsound (the byte move drops bits 24-25 of x);
// the sound construction is the 4-lane generalisation of the landed 16-bit
// amount-6 family. Full derivation and measured word counts are in
// docs/41-superopt-spike-findings.md, Target 3; the tests below are the
// gate that keeps those claims honest.

/// A much wider operand set than `sample_words`, for the fused forms that
/// actually win and so will be wired: every value in each byte lane
/// independently (4 * 256 cases, catching any per-lane mask or index error),
/// a few cross-lane patterns, and a deterministic pseudo-random sweep for
/// the carry interactions across lane boundaries. Still not the full 2^32
/// domain (days at this crate's per-case cost), but structurally exhaustive
/// in the dimension a fixed construction can get wrong: which bits of which
/// byte land where.
fn swept_words() -> Vec<[u8; 4]> {
    let mut v: Vec<[u8; 4]> = Vec::new();
    // Baseline pattern with distinctive bytes in every position.
    let base = [0x12u8, 0x34, 0x56, 0x78];
    for lane in 0..4usize {
        for b in 0..=u8::MAX {
            let mut w = base;
            w[lane] = b;
            v.push(w);
        }
    }
    // Boundary and alternating patterns.
    for w in [
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        [0x01, 0x00, 0x00, 0x80],
        [0x80, 0x00, 0x00, 0x01],
        [0xAA, 0x55, 0xAA, 0x55],
    ] {
        v.push(w);
    }
    // Deterministic LCG: no dependency on a thread RNG, reproducible across
    // runs so a failure is always replayable.
    let mut state: u32 = 0x1234_5678;
    for _ in 0..4096 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push(state.to_le_bytes());
    }
    v
}

/// The general 4-lane left-shift family: rotate every byte right by `8 - r`
/// with `RRNCF`, then recombine high-to-low. The `MOVLW` literals are static
/// because `Candidate` holds `&'static str`.
fn construction_shl_family(r: u32) -> Candidate {
    // (rotate count, MOVLW hi, MOVLW lo) per amount with a fused form.
    let (rot, hi, lo) = match r {
        6 => (2, "movlw 0xC0", "andlw 0x3F"),
        7 => (1, "movlw 0x80", "andlw 0x7F"),
        _ => unreachable!("only the amounts with a fused form"),
    };
    let mut c: Candidate = Vec::new();
    for _ in 0..rot {
        for a in [
            "rrncf 0x023,F,A",
            "rrncf 0x022,F,A",
            "rrncf 0x021,F,A",
            "rrncf 0x020,F,A",
        ] {
            c.push(a);
        }
    }
    // Combine high-to-low so each lane reads its pristine lower neighbour
    // before that neighbour is rewritten.
    for b in (1..4usize).rev() {
        // Lane `b` lives at 0x020 + b; its lower neighbour at 0x020 + b - 1.
        c.push(hi);
        c.push(["andwf 0x021,F,A", "andwf 0x022,F,A", "andwf 0x023,F,A"][b - 1]);
        c.push(["movf 0x020,W,A", "movf 0x021,W,A", "movf 0x022,W,A"][b - 1]);
        c.push(lo);
        c.push(["iorwf 0x021,F,A", "iorwf 0x022,F,A", "iorwf 0x023,F,A"][b - 1]);
    }
    c.push(hi);
    c.push("andwf 0x020,F,A");
    c
}

#[test]
fn left_shift_32bit_by_6_fused_form_wins() {
    let c = construction_shl_family(6);
    assert_eq!(c.len(), 25, "4*2 rotate + 3*5 combine + 2");
    assert!(
        verify(
            &c,
            &cases_with(u32::wrapping_shl, 6, &swept_words(), W_SAMPLE)
        ),
        "the fused amount-6 form must be correct over the sample"
    );
    assert!(c.len() < 5 * 6, "25 words beats the 30-word unroll");
}

#[test]
fn left_shift_32bit_by_7_fused_form_wins() {
    let c = construction_shl_family(7);
    assert_eq!(c.len(), 21, "4*1 rotate + 3*5 combine + 2");
    assert!(
        verify(
            &c,
            &cases_with(u32::wrapping_shl, 7, &swept_words(), W_SAMPLE)
        ),
        "the fused amount-7 form must be correct over the sample"
    );
    assert!(c.len() < 5 * 7, "21 words beats the 35-word unroll");
}

#[test]
fn the_tickets_amount7_byte_move_form_is_unsound() {
    // The decomposition the ticket proposed, implemented as a byte move plus
    // a rotate. It must FAIL verification: that failure is the reason the
    // family form above exists. If this ever passes, the sample has gone
    // blind to the lost-bits witness and needs widening.
    let c: Candidate = vec![
        // x <<= 8 as a byte move: dst[3..1] = src[2..0], dst[0] = 0
        "movff 0x022,0x023",
        "movff 0x021,0x022",
        "movff 0x020,0x021",
        "clrf 0x020,A",
        // >>= 1
        "bcf 0xFD8,0,A",
        "rrcf 0x023,F,A",
        "rrcf 0x022,F,A",
        "rrcf 0x021,F,A",
        "rrcf 0x020,F,A",
    ];
    assert!(
        !verify(
            &c,
            &cases_with(u32::wrapping_shl, 7, &swept_words(), W_SAMPLE)
        ),
        "the byte-move form drops bits 24-25 of x and must not verify"
    );
}
