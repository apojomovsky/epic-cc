//! Whole-program validation for the PIC8 pipeline: checks what `llvm-link`
//! lets through on the merged module. Expects N translation units already
//! merged into one `.ll` by `llvm-link` (docs/31 §7); this stage does not link.

use ir::{Inst, Module, SrcLoc};
use std::collections::{BTreeMap, BTreeSet};

/// Validates the merged module and hands it on unchanged.
///
/// Panics when the entry invariant breaks: empty module, missing or duplicate
/// `main`, or a call target with no definition.
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
/// Downstream that becomes a CALL to an undefined assembler label, so this
/// check raises the error while the names are still the user's.
fn check_calls_resolved(m: &Module) {
    let defined: BTreeSet<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    let mut missing: BTreeMap<&str, Vec<&SrcLoc>> = BTreeMap::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    if c.func.chars().all(|ch| ch.is_ascii_digit()) {
                        continue;
                    }
                    // An `llvm.*` intrinsic is `declare`d by clang without a
                    // definition here; legalize lowers every supported one and
                    // panics on an unknown, so skipping keeps this a
                    // user-symbol check.
                    if c.func.starts_with("llvm.") {
                        continue;
                    }
                    if !defined.contains(c.func.as_str()) {
                        let sites = missing.entry(c.func.as_str()).or_default();
                        if let Some(loc) = &c.loc {
                            sites.push(loc);
                        }
                    }
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "wholeprog: undefined symbols: {}",
        missing
            .iter()
            .map(|(name, locs)| symbol_with_sites(name, locs))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Formats one undefined-symbol entry. Appends call sites when the module
/// carries debug locations; emits the bare symbol name otherwise.
fn symbol_with_sites(name: &str, locs: &[&SrcLoc]) -> String {
    if locs.is_empty() {
        return name.to_string();
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let sites: Vec<String> = locs
        .iter()
        .map(|l| l.to_string())
        .filter(|s| seen.insert(s.clone()))
        .collect();
    format!("{name} (called at {})", sites.join(", "))
}
