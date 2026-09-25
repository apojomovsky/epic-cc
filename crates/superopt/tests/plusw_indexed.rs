//! The indexed lowering (epic-cc#665): a byte index into a small
//! static array reads and writes through `PLUSW0` off one resident
//! `LFSR`, instead of the 16-bit FSR add around `INDF0`.
//!
//! Contract: over the valid domain (index below the array size, size at
//! most 128, where the signed `PLUSW0` offset agrees with unsigned
//! address arithmetic) the read leaves `array[idx]` in `W` and the
//! store writes its value byte to `array[idx]`, `FSR0` unmoved in both
//! cases. Out-of-bounds indices are C UB and outside the contract.

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case};

const BASE: usize = 0x200;
const IDX: usize = 0x030;
const VAL: usize = 0x031;
const FSR0L: usize = 0xFE9;
const FSR0H: usize = 0xFEA;

fn read_candidate() -> Candidate {
    vec!["lfsr 0, 0x200", "movf 0x030,W,A", "movf 0xFEB,W,A"]
}

fn store_candidate() -> Candidate {
    vec!["lfsr 0, 0x200", "movf 0x030,W,A", "movff 0x031,0xFEB"]
}

/// Cases over the full valid index domain with two array contents
/// (identity and complement), each case checking the accessed lane
/// and that `FSR0` never moved.
fn read_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for pattern in [0u8, 1] {
        for idx in 0..128usize {
            let mut pokes: Vec<(usize, u8)> = (0..128)
                .map(|b| {
                    let v = if pattern == 0 { b as u8 } else { !(b as u8) };
                    (BASE + b, v)
                })
                .collect();
            pokes.push((IDX, idx as u8));
            let want = if pattern == 0 {
                idx as u8
            } else {
                !(idx as u8)
            };
            cases.push(Case {
                entry_w: 0,
                pokes,
                allowed_changes: vec![FSR0L, FSR0H],
                check: Box::new(move |sim: &Pic18| {
                    sim.w() == want && sim.ram()[FSR0L] == 0x00 && sim.ram()[FSR0H] == 0x02
                }),
            });
        }
    }
    cases
}

fn store_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for idx in 0..128usize {
        let mut pokes: Vec<(usize, u8)> = (0..128).map(|b| (BASE + b, 0)).collect();
        pokes.push((IDX, idx as u8));
        pokes.push((VAL, 0xA0 + (idx as u8) % 16));
        let want = 0xA0 + (idx as u8) % 16;
        cases.push(Case {
            entry_w: 0,
            pokes,
            allowed_changes: vec![BASE + idx, FSR0L, FSR0H],
            check: Box::new(move |sim: &Pic18| {
                sim.ram()[BASE + idx] == want
                    && sim.w() == idx as u8
                    && sim.ram()[FSR0L] == 0x00
                    && sim.ram()[FSR0H] == 0x02
            }),
        });
    }
    cases
}

#[test]
fn plusw0_indexed_read_leaves_the_lane_in_w() {
    let c = read_candidate();
    assert_eq!(c.len(), 3, "LFSR + index load + PLUSW0 read");
    assert!(
        verify(&c, &read_cases()),
        "the indexed read must leave array[idx] in W over 0..128"
    );
}

#[test]
fn plusw0_indexed_store_writes_only_the_lane() {
    let c = store_candidate();
    assert_eq!(c.len(), 3, "LFSR + index load + MOVFF store");
    assert!(
        verify(&c, &store_cases()),
        "the indexed store must write only array[idx] over 0..128"
    );
}
