//! Target 2 (epic-cc#514, epic-cc#505): `x <<= 4` on a single byte,
//! in place. epic-cc#505's repro is 16-bit (`x << 4` on `unsigned int`,
//! 12 words: four steps of `BCF STATUS,C` + `RLCF`/`RLCF`). This spike
//! scopes down to the 8-bit, single-register case rather than the full
//! 16-bit contract: the unrolled baseline for one byte's worth of that same
//! pattern is 4 steps of `BCF`+`RLCF`, 8 words, and the nibble-swap trick
//! below is a byte-local operation, so it is the natural first slice to
//! verify before claiming anything about the 16-bit generalization (left
//! as follow-up, see docs/41).
//!
//! Destination/source register: 0x020 (arbitrary access-bank GPR, shifted
//! in place, matching how `x <<= n` compiles: same address in and out).

use pic14_sim::Pic18;
use superopt::{shortest, Case, STATUS_ADDR, STATUS_C_BIT};

const REG: usize = 0x020;

/// A curated, non-exhaustive set of (low nibble, high nibble) pairs crossed
/// with entry `W` and entry `C`, dimensions a correct candidate must not
/// depend on (the alphabet includes `RLCF`, which reads `C`, and several
/// `W`-touching instructions; epic-cc#514's review found the original
/// version of this file fixed both at their sim-default values instead of
/// varying them, which would have hidden a candidate that only worked by
/// accident). Low nibbles 0x0, 0x1, 0x7, 0x8, 0xF cover zero, a lone low
/// bit, a lone high bit within the nibble, and all-ones; high nibbles
/// 0x0/0x3/0x8/0xF catch a candidate that lets the incoming high nibble
/// leak into the result (it must not: shifted-out bits are gone, not
/// wrapped).
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for lo in [0x0u8, 0x1, 0x7, 0x8, 0xF] {
        for hi in [0x0u8, 0x3, 0x8, 0xF] {
            for w in [0x00u8, 0xFF, 0x2A] {
                for c in [false, true] {
                    let v = (hi << 4) | lo;
                    let expect = (lo << 4) & 0xFF;
                    let status = if c { STATUS_C_BIT } else { 0 };
                    cases.push(Case {
                        entry_w: w,
                        pokes: vec![(REG, v), (STATUS_ADDR, status)],
                        allowed_changes: &[REG],
                        check: Box::new(move |sim: &Pic18| sim.ram()[REG] == expect),
                    });
                }
            }
        }
    }
    cases
}

/// Curated alphabet: the nibble-swap family (`SWAPF`) plus mask literals
/// relevant to isolating a nibble, not the full PIC18 opcode/literal space.
/// See docs/41 for why this is a deliberate scoping choice, not an
/// oversight: masks for nibble/byte-boundary bit operations are naturally
/// power-of-two shaped, so a handful of curated literals covers the
/// plausible solutions without enumerating all 256.
const ALPHABET: &[&str] = &[
    "swapf 0x020,W,A",
    "swapf 0x020,F,A",
    "andlw 0xF0",
    "andlw 0x0F",
    "iorlw 0xF0",
    "movwf 0x020,A",
    "movf 0x020,W,A",
    "andwf 0x020,F,A",
    "iorwf 0x020,F,A",
    "clrf 0x020,A",
    "bcf 0xFD8,0,A",
    "rlcf 0x020,F,A",
    "rlcf 0x020,W,A",
];

#[test]
fn shortest_byte_shift_left_4() {
    let cases = cases();
    let hits = shortest(ALPHABET, &cases, 4);
    assert!(
        !hits.is_empty(),
        "no verified candidate up to length 4 in this curated alphabet"
    );
    let len = hits[0].len();
    eprintln!(
        "shift-left-4 (8-bit): {} word(s), {} candidate(s) at that length \
         (unrolled BCF+RLCF baseline for one byte: 8 words)",
        len,
        hits.len()
    );
    for hit in &hits {
        eprintln!("  {hit:?}");
    }
    assert!(
        len < 8,
        "expected to beat the 8-word unrolled single-byte baseline, got {len}"
    );
}
