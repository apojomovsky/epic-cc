//! Whole-program validation for the PIC8 pipeline: N translation units have
//! already been merged into one `.ll` by `llvm-link` (see docs/31 D-7), so
//! this stage does not link. It checks what `llvm-link` lets through.

use ir::{Inst, Module};
use std::collections::BTreeSet;

/// Validate the merged module and hand it on unchanged.
///
/// Panics if the module has no functions, if it does not contain exactly one
/// `main`, or if any call target has no definition.
pub fn merge(m: Module) -> Module {
    assert!(!m.funcs.is_empty(), "wholeprog: no functions in module");
    check_entry(&m);
    check_calls_resolved(&m);
    m
}

fn check_entry(m: &Module) {
    let mains = m.funcs.iter().filter(|f| f.name == "main").count();
    assert_eq!(
        mains, 1,
        "wholeprog: expected exactly one `main`, found {mains}"
    );
}

/// `llvm-link` leaves an unsatisfied `declare` in place rather than failing.
/// Downstream that becomes a CALL to a label the assembler never heard of, so
/// the error has to be raised here, while the names are still the user's.
fn check_calls_resolved(m: &Module) {
    let defined: BTreeSet<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    let mut missing: BTreeSet<&str> = BTreeSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    if !defined.contains(c.func.as_str()) {
                        missing.insert(c.func.as_str());
                    }
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "wholeprog: undefined symbols: {}",
        missing.into_iter().collect::<Vec<_>>().join(", ")
    );
}
