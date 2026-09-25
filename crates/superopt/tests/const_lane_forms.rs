//! The wide-const single-word lanes (epic-cc#666): a `0xFF` lane
//! stores through `SETF` instead of a two-word `MOVLW`/`MOVWF` pair.
//!
//! Contract: the lane lands exactly, nothing else in RAM moves, and
//! `W` survives (the pair it replaces left `W` holding the literal,
//! so callers must not depend on that; the lowering never does).

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case};

#[test]
fn setf_writes_ones_and_preserves_w() {
    let c: Candidate = vec!["setf 0x020,A"];
    let mut cases = Vec::new();
    for v in [0x00u8, 0x55, 0xFF] {
        cases.push(Case {
            entry_w: 0xA5,
            pokes: vec![(0x020, v)],
            allowed_changes: vec![0x020],
            check: Box::new(|sim: &Pic18| sim.ram()[0x020] == 0xFF && sim.w() == 0xA5),
        });
    }
    assert!(
        verify(&c, &cases),
        "SETF must write 0xFF, move nothing else, and preserve W"
    );
}
