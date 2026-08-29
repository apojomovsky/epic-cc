//! Whole-program validation for the PIC8 pipeline: N translation units have
//! already been merged into one `.ll` by `llvm-link` (see docs/31 D-7), so
//! this stage does not link. It checks what `llvm-link` lets through.

use ir::{Inst, Module, SrcLoc};
use std::collections::{BTreeMap, BTreeSet};

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
    let mut missing: BTreeMap<&str, Vec<&SrcLoc>> = BTreeMap::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    if c.func.chars().all(|ch| ch.is_ascii_digit()) {
                        continue;
                    }
                    // An `llvm.*` intrinsic is `declare`d by clang, never
                    // defined here; legalize lowers every supported one and
                    // panics loudly on an unknown, so skipping keeps this a
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

/// One entry of the undefined-symbols message. The referencing call sites
/// ride along when the module carries debug locations; without them the
/// message stays the bare symbol name.
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
