//! Target 1 (epic-cc#514, epic-cc#501): materialize `dst = (Z ? 1 : 0)`
//! after some prior comparison has already set STATUS.Z. #501's repro
//! (`y = (x == 4u)`) currently lowers to, word for word:
//!
//! ```text
//! BRA tmp0 / MOVLW 0x00 / BRA tmp2 / MOVLW 0x01 / MOVWF dst
//! ```
//!
//! 4 words for the diamond plus the `MOVWF` that stores it, 5 words total;
//! the ticket's own text is explicit that the diamond count (4) excludes
//! that store. This spike's contract always ends with the value in RAM
//! (the diamond is only useful once stored), so every count in this file
//! is "including the store", and the isel-pic18 baseline to beat is **5**,
//! not 4.
//!
//! The ticket also names a hand-derived 3-word candidate, `CLRF dst; B<inv>
//! skip; INCF dst,F`. **It does not verify**: `CLRF` always sets Z (the
//! result of clearing anything to zero is zero), so it destroys the very
//! Z bit the branch right after it needs to read. Confirmed both by this
//! search (candidates built from `clrf` never verify against a case set
//! that varies incoming Z independently of the `clrf`) and by hand: `CLRF
//! dst` given entry Z=0 leaves Z=1 after the `CLRF`, so `B<inv>` reads the
//! wrong flag. Landing that fix as literally written would be a
//! miscompile, not a density win; see docs/41-superopt-spike-findings.md.
//!
//! Destination register: 0x020 (an arbitrary access-bank GPR; which
//! physical address `alloc` actually picks does not affect which
//! instruction sequence is shortest).

use pic14_sim::Pic18;
use superopt::{shortest, Case, STATUS_ADDR, STATUS_C_BIT, STATUS_Z_BIT};

const DST: usize = 0x020;

/// Every (Z, C, entry-W, poisoned-dst) combination this contract must
/// survive. Z is the actual signal; C and entry W are dimensions a correct
/// candidate must not depend on. `entry_w` reaches the machine via
/// `Case::entry_w` (`Pic18::set_w`), not a RAM poke: an earlier version of
/// this file looped over W values without ever applying them, which made
/// every candidate that reads entry W (e.g. `MOVWF dst` with no preceding
/// `MOVLW`) a silent false positive, caught in epic-cc#514's review. The
/// destination starts poisoned to a value that is wrong for *both*
/// outcomes (0x55), so a candidate that silently leaves it untouched also
/// fails.
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
                    entry_w: w,
                    pokes: vec![(STATUS_ADDR, status), (DST, 0x55)],
                    check: Box::new(move |sim: &Pic18| sim.ram()[DST] == expect),
                });
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
    let hits = shortest(ALPHABET, &cases, 5);
    assert!(
        !hits.is_empty(),
        "no verified candidate up to length 5; isel-pic18's own 5-word diamond+store \
         would then already be shortest-known within this alphabet"
    );
    let len = hits[0].len();
    eprintln!(
        "bool-materialize (incl. store): {} word(s), {} candidate(s) at that length \
         (isel-pic18 today: 5 words, diamond + MOVWF)",
        len,
        hits.len()
    );
    for hit in &hits {
        eprintln!("  {hit:?}");
    }
    assert!(
        len < 5,
        "expected the search to beat isel-pic18's current 5-word diamond+store, got {len}"
    );
}
