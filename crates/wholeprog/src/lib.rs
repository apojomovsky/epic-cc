//! Whole-program validation for the PIC8 pipeline: checks what `llvm-link`
//! lets through on the merged module. Expects N translation units already
//! merged into one `.ll` by `llvm-link` (docs/31 §7); this stage does not link.

use ir::{collect_global_vals, GepBase, Inst, Module, SrcLoc, Val};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Validates the merged module and hands it on.
///
/// Maps the C++ entry first: a lone `_Z4mainv` (what `int main()` lowers
/// to on the `.cpp` path, epic-cc#457) with no plain `main` is renamed to
/// `main`, so every stage past this boundary keeps its single-entry
/// spelling. A `main` plus a `_Z4mainv` still counts two and fails.
///
/// Panics when the entry invariant breaks: empty module, missing or duplicate
/// `main`, or a call target or data symbol with no definition.
pub fn merge(mut m: Module) -> Module {
    map_cpp_entry(&mut m);
    assert!(!m.funcs.is_empty(), "wholeprog: no functions in module");
    check_entry(&m);
    check_calls_resolved(&m);
    check_globals_resolved(&m);
    m
}

fn map_cpp_entry(m: &mut Module) {
    if m.funcs.iter().any(|f| f.name == "main") {
        return;
    }
    if m.funcs.iter().filter(|f| f.name == "_Z4mainv").count() == 1 {
        m.funcs
            .iter_mut()
            .find(|f| f.name == "_Z4mainv")
            .expect("counted one above")
            .name = "main".to_string();
    }
}

fn check_entry(m: &Module) {
    let mains = m
        .funcs
        .iter()
        .filter(|f| f.name == "main" || f.name == "_Z4mainv")
        .count();
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
                    // user-symbol check. `_delay` is the same shape: the user
                    // declares it with no definition and isel expands the
                    // call inline (epic-cc#700).
                    if c.func.starts_with("llvm.") || c.func == "_delay" {
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

/// `llvm-link` leaves an unsatisfied `external` data declaration in place
/// rather than failing (irparse skips declarations, epic-cc#909).
/// Downstream that becomes a load from an undefined assembler label, so
/// this check raises the error while the name is still the user's. A use
/// of a function label (`ptr @handler`) counts as defined by `funcs`.
fn check_globals_resolved(m: &Module) {
    let defined: BTreeSet<&str> = m
        .globals
        .iter()
        .map(|g| g.name.as_str())
        .chain(m.funcs.iter().map(|f| f.name.as_str()))
        .collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for g in &m.globals {
        for (_, target, _) in &g.refs {
            used.insert(target.clone());
        }
    }
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                let mut vals = HashSet::new();
                collect_global_vals(inst, &mut vals);
                used.extend(vals.into_iter());
                // `collect_global_vals` skips these shapes (it is shared
                // with legalize, whose view must not change); the Val and
                // pointer-string operands are read here instead.
                match inst {
                    Inst::Load(l) => {
                        if let Some(n) = l.ptr.strip_prefix('@') {
                            used.insert(n.to_string());
                        }
                    }
                    Inst::Store(s) => {
                        if let Some(n) = s.ptr.strip_prefix('@') {
                            used.insert(n.to_string());
                        }
                    }
                    Inst::Gep(g) => {
                        if let GepBase::Global(n) = &g.base {
                            used.insert(n.clone());
                        }
                    }
                    Inst::BrCond(b) => {
                        if let Val::Global(n) = &b.cond {
                            used.insert(n.clone());
                        }
                    }
                    Inst::Switch(s) => {
                        if let Val::Global(n) = &s.val {
                            used.insert(n.clone());
                        }
                    }
                    Inst::Asm(a) => {
                        for o in &a.operands {
                            if let Some(n) = o.ptr.strip_prefix('@') {
                                used.insert(n.to_string());
                            }
                        }
                    }
                    Inst::VaArg(v) => {
                        if let Some(n) = v.ptr.strip_prefix('@') {
                            used.insert(n.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let missing: Vec<&String> = used
        .iter()
        .filter(|n| !defined.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "wholeprog: undefined symbols: {}",
        missing
            .iter()
            .map(|n| n.to_string())
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
