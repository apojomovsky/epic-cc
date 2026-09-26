//! Cycle-exact `_delay(cycles)` count planner shared by all four backends.
//!
//! Every core burns the same loop shape: an 8-bit down-counter running
//! `DECFSZ cnt,F` (1 cycle, 2 on the final skip) plus a 2-cycle branch
//! back (`GOTO` on PIC14/PIC14E/baseline, `BRA` on PIC18), with a 2-cycle
//! `MOVLW`/`MOVWF` setup per level. One level holding count `c`
//! (1..=256, where 256 is encoded as literal 0 and wraps the full byte)
//! costs exactly `3*c + 1`; each enclosing level adds its setup plus 3
//! cycles per inner run (2 on its last). `plan_delay` solves counts plus
//! 0-3 trailing `NOP`s for any request up to three nested levels; larger
//! requests panic naming the ceiling instead of emitting wrong code.

/// A solved delay: sequential loop nests (each a list of outer-first
/// counts, every count 1..=256) followed by trailing `NOP`s. Fragments
/// run back to back and reuse the same counter bytes, so the deepest
/// single nest bounds the fixed-region demand at 3 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayPlan {
    pub nests: Vec<Vec<u16>>,
    pub tail_nops: u8,
}

/// Cost of one nest in instruction cycles: innermost `3*c + 1`, each
/// outer level `o * (inner + 3) + 1` (the `+3` is the inner run's share
/// of the outer `DECFSZ`/`GOTO`, `+1` the outer setup net of the saved
/// final branch).
fn nest_cost(levels: &[u16]) -> u64 {
    let mut total = 3 * u64::from(*levels.last().expect("iselcore: empty delay nest")) + 1;
    for &o in levels[..levels.len() - 1].iter().rev() {
        total = u64::from(o) * (total + 3) + 1;
    }
    total
}

/// Exact cost of a solved plan: every nest plus the trailing `NOP`s.
pub fn delay_cost(plan: &DelayPlan) -> u64 {
    plan.nests.iter().map(|n| nest_cost(n)).sum::<u64>() + u64::from(plan.tail_nops)
}

/// Largest single nest: two full levels under one outer (`o * (3*256+4) + 1`
/// at `o = 256`).
const L2_MAX: u64 = 256 * (3 * 256 + 4) + 1;
/// Largest tail a third level can leave behind: anything `plan_delay`
/// solves without a third level.
const L3_TAIL_MAX: u64 = L2_MAX + 771;
/// Innermost pair fixed at full counts: `F2(256, 256) + 3` per outer run.
const L3_STEP: u64 = L2_MAX + 3;
/// Ceiling: three full levels plus the largest leftover tail.
pub const DELAY_MAX: u64 = 256 * L3_STEP + 1 + L3_TAIL_MAX;

/// Solve `cycles` into loop nests plus trailing `NOP`s. Panics on a
/// request past `DELAY_MAX` (about four seconds at 48 MHz).
pub fn plan_delay(cycles: u64) -> DelayPlan {
    assert!(
        cycles <= DELAY_MAX,
        "iselcore: _delay({cycles}) exceeds the {DELAY_MAX}-cycle ceiling"
    );
    let mut plan = DelayPlan {
        nests: Vec::new(),
        tail_nops: 0,
    };
    let mut rest = cycles;
    if rest > L3_TAIL_MAX {
        let a = (1..=256u64)
            .find(|&a| rest <= a * L3_STEP + 1 + L3_TAIL_MAX)
            .expect("iselcore: third-level search must hit inside the ceiling");
        rest -= a * L3_STEP + 1;
        plan.nests.push(vec![a as u16, 256, 256]);
    }
    if rest > 771 {
        // Two-level fragment: the first outer count whose span reaches
        // `rest`, then the largest inner count that still fits. The
        // leftover (under one outer step) falls through to the single
        // loop below.
        for o in 1u64..=256 {
            if rest > 772 * o + 772 {
                continue;
            }
            let mut i = ((rest - 1 - o) / (3 * o)).min(256);
            while o * (3 * i + 4) + 1 > rest {
                i -= 1;
            }
            rest -= o * (3 * i + 4) + 1;
            plan.nests.push(vec![o as u16, i as u16]);
            break;
        }
    }
    if rest >= 4 {
        let c = (rest - 1) / 3;
        plan.nests.push(vec![c as u16]);
        rest -= 3 * c + 1;
    }
    plan.tail_nops = rest as u8;
    plan
}
