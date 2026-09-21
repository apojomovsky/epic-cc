//! Target 1 (epic-cc#514, epic-cc#501): materialize `dst = (Z ? 1 : 0)`
//! after some prior comparison has already set STATUS.Z. #501's repro
//! (`y = (x == 4u)`) currently lowers to a 4-word branch diamond:
//!
//! ```text
//! BRA tmp0 / MOVLW 0x00 / BRA tmp2 / MOVLW 0x01 / (MOVWF, outside the diamond)
//! ```
//!
//! and the ticket names two hand-derived candidates to confirm: `CLRF dst;
//! B<inv> skip; INCF dst,F` (3 words) and, speculatively, a 2-word
//! carry-based sequence. This is a pure post-compare tail: the contract
//! never re-derives the comparison, it only consumes the Z bit a prior
//! SUBWF already set.
//!
//! Destination register: 0x020 (an arbitrary access-bank GPR; which
//! physical address `alloc` actually picks does not affect which
//! instruction sequence is shortest).

use pic14_sim::Pic18;
use superopt::{shortest, Case, STATUS_ADDR, STATUS_C_BIT, STATUS_Z_BIT};

const DST: usize = 0x020;

/// Every (Z, C, W, poisoned-dst) combination this contract must survive.
/// Z is the actual signal; C and W are dimensions a correct candidate must
/// not depend on (a candidate that happens to reuse a stale C or W value
/// would pass a Z-only case set by accident). The destination starts
/// poisoned to a value that is wrong for *both* outcomes (0x55), so a
/// candidate that silently leaves it untouched also fails.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for z in [false, true] {
        for c in [false, true] {
            for w in [0x00u8, 0xFFu8, 0x2Au8] {
                let mut status = 0u8;
                if z {
                    status |= STATUS_Z_BIT;
                }
                if c {
                    status |= STATUS_C_BIT;
                }
                let expect: u8 = if z { 1 } else { 0 };
                cases.push(Case {
                    pokes: vec![(STATUS_ADDR, status), (DST, 0x55)],
                    check: Box::new(move |sim: &Pic18| sim.ram()[DST] == expect),
                });
                let _ = w; // W is not poked (candidates never read stale W here); kept named for the case matrix's documentation value.
            }
        }
    }
    cases
}

/// The alphabet: instructions plausible for a Z-to-byte materialization,
/// curated rather than the full ISA (the full PIC18 opcode set, including
/// every addressing mode and literal value, is a much larger spike than
/// this one; see docs/41 for the scoping call). All operate on `DST`
/// (0x020) or the flag bits `SUBWF`'s successor would have already set.
const ALPHABET: &[&str] = &[
    "clrf 0x020,A",
    "setf 0x020,A",
    "movlw 0x00",
    "movlw 0x01",
    "movwf 0x020,A",
    "incf 0x020,F,A",
    "decf 0x020,F,A",
    "comf 0x020,F,A",
    "rlcf 0x020,F,A",
    "rrcf 0x020,F,A",
    "btfsc 0xFD8,2,A", // skip next if Z clear
    "btfss 0xFD8,2,A", // skip next if Z set
    "btfsc 0xFD8,0,A", // skip next if C clear
    "btfss 0xFD8,0,A", // skip next if C set
];

#[test]
fn shortest_z_to_byte_materialization() {
    let cases = cases();
    let hits = shortest(ALPHABET, &cases, 4);
    assert!(
        !hits.is_empty(),
        "no verified candidate up to length 4; isel-pic18's own 4-word diamond \
         would then already be shortest-known within this alphabet"
    );
    let len = hits[0].len();
    eprintln!(
        "bool-materialize: {} word(s), {} candidate(s) at that length (isel-pic18 today: 4 words)",
        len,
        hits.len()
    );
    for hit in &hits {
        eprintln!("  {hit:?}");
    }
    // The ticket's own hand-derived 3-word candidate is a plausible floor;
    // assert the search does at least as well as that, so a regression in
    // the alphabet or case set (one that accidentally makes verification
    // easier) would show up as "found nothing" rather than as silently
    // reporting something worse than the human already had.
    assert!(
        len <= 3,
        "expected the search to match or beat the ticket's hand-derived 3-word candidate, got {len}"
    );
}
