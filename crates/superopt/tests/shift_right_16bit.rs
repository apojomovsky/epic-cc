//! Target 3, 16-bit lane pair (epic-cc#526): constant-count RIGHT shifts.
//! `isel-pic18` lowers them as `r` steps of `BCF STATUS,C` + `RRCF lo` +
//! `RRCF hi`, 3 words/step, so the baseline is `3*r` words with no
//! whole-byte shortcut (that is `m > 0`, handled by the byte-move path).
//!
//! Right shifts are the mirror of the left-shift target in
//! `shift_16bit.rs`, and the mirrors of its constructions are the claims
//! here. As with the left side, a 16-bit amount-4 form exists (9 words
//! against a 12-word baseline) and the search bound cannot reach it, so
//! each candidate is constructed and `verify()`d over the full 65536
//! (`lo, hi`) domain plus a `W` sample. Nothing here is wired into
//! `isel-pic18` from a hand derivation; docs/41's unsound `CLRF`-first
//! candidate (epic-cc#501) is the reason.

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const LO: usize = 0x020;
const HI: usize = 0x021;
const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Right-shift cases over `pairs` x `w_values` x both `C` values. Mirror of
/// `shift_16bit.rs::cases_over` with `wrapping_shr` and the low-to-high
/// rotate order the direction implies.
fn cases_over(amount: u32, pairs: &[(u8, u8)], w_values: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    for &(lo, hi) in pairs {
        for &w in w_values {
            for c in [false, true] {
                let x = (u16::from(hi) << 8) | u16::from(lo);
                let expect = x.wrapping_shr(amount);
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

fn cases_exhaustive(amount: u32) -> Vec<Case> {
    let pairs: Vec<(u8, u8)> = (0..=255u16)
        .flat_map(|lo| (0..=255u16).map(move |hi| (lo as u8, hi as u8)))
        .collect();
    cases_over(amount, &pairs, &[0x00])
}

fn verify_construction(amount: u32, candidate: &Candidate) -> bool {
    let pairs: Vec<(u8, u8)> = vec![
        (0x00, 0x00),
        (0xFF, 0xFF),
        (0x0F, 0xF0),
        (0xF0, 0x0F),
        (0x01, 0x80),
        (0x12, 0x34),
        (0xAB, 0xCD),
        (0x55, 0xAA),
    ];
    verify(candidate, &cases_exhaustive(amount))
        && verify(candidate, &cases_over(amount, &pairs, W_SAMPLE))
}

/// 16-bit right shift by 4, W-only (no scratch byte, the same precondition
/// as the landed left-shift forms). Derivation: `lo' = (lo>>4) | ((hi&0x0F)<<4)`
/// and `hi' = hi>>4`, both expressible as nibble-swaps plus a mask. Nine
/// words against the 12-word unroll.
fn construction_r4() -> Candidate {
    vec![
        "swapf 0x020,F,A", // lo = nibble-swapped orig lo
        "movlw 0x0F",
        "andwf 0x020,F,A", // lo = orig_lo >> 4 (low nibble only)
        "swapf 0x021,W,A", // W = nibble-swapped hi
        "andlw 0xF0",      // W = (orig_hi & 0x0F) << 4
        "iorwf 0x020,F,A", // lo' complete
        "swapf 0x021,W,A", // W = nibble-swapped hi
        "andlw 0x0F",      // W = orig_hi >> 4 = hi'
        "movwf 0x021,A",
    ]
}

#[test]
fn right_shift_16bit_amount4_is_exact() {
    let c = construction_r4();
    assert!(
        verify_construction(4, &c),
        "verified 16-bit right-shift-by-4 form must hold over the full domain"
    );
    assert!(c.len() < 12, "verified form must beat the 12-word unroll");
}

#[test]
fn right_shift_16bit_amount4_beats_the_unroll() {
    // The comparison the wiring decision rests on. The exactness test above
    // is the gate; this makes the payoff explicit so it cannot silently
    // become a no-op if the unroll's cost ever changes.
    assert!(
        construction_r4().len() < 3 * 4,
        "must beat 3*r unroll words"
    );
}
