//! Whole-program validation and dead-global pruning for the PIC8 pipeline:
//! checks what `llvm-link` lets through on the merged module, then drops
//! `internal` globals nothing references so alloc never reserves RAM for
//! them. Expects N translation units already merged into one `.ll` by
//! `llvm-link` (docs/31 §7); this stage does not link.

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
    drop_unreferenced_internals(&mut m);
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
    let mut used = code_referenced_globals(m);
    for g in &m.globals {
        for (_, target, _) in &g.refs {
            used.insert(target.clone());
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

/// Marks globals named literally in verbatim asm text: the exact spelling
/// and the XC8 underscore alias (`_x` addresses C static `x`, asm crate's
/// `insert_label`). Over-approximates by construction (any identifier
/// token that could name a global keeps it); keeping is always safe.
fn mark_text_refs(names: &HashSet<&str>, used: &mut BTreeSet<String>, text: &str) {
    for tok in
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
    {
        if names.contains(tok) {
            used.insert(tok.to_string());
        } else if let Some(bare) = tok.strip_prefix('_') {
            if names.contains(bare) {
                used.insert(bare.to_string());
            }
        }
    }
}

/// Every global named by code: loads, stores, GEP bases, `Val::Global`
/// operands, inline-asm templates, and module-level asm text. Debug
/// metadata names nothing here (irparse drops it), so a global this
/// misses is dead to codegen, not just unseen. Shared by the
/// undefined-symbol check and the drop below.
fn code_referenced_globals(m: &Module) -> BTreeSet<String> {
    let names: HashSet<&str> = m.globals.iter().map(|g| g.name.as_str()).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for entry in &m.module_asm {
        mark_text_refs(&names, &mut used, entry);
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
                        // A template can name a global literally
                        // (`asm("banksel _m_add_a")` carries no operand), so
                        // identifier tokens count too: the exact spelling
                        // and the XC8 underscore alias (`_x` addresses C
                        // static `x`, asm crate's `insert_label`).
                        mark_text_refs(&names, &mut used, &a.template);
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
    used
}

/// Drops `internal` globals with no code references, before alloc.
///
/// `opt` folds the dead test functions but keeps their `static` globals,
/// which then hold RAM through the whole build. `internal` linkage proves
/// no outside observer can name them, so reachability from code decides:
/// roots are the code references plus every externally visible global,
/// liveness follows initializer `refs` to a fixpoint, the rest go.
/// Flash tables drop by the same rule: the fuse words leave the hex
/// through the driver's own config emission, never through this global.
fn drop_unreferenced_internals(m: &mut Module) {
    let mut live: BTreeSet<String> = m
        .globals
        .iter()
        .filter(|g| !g.internal)
        .map(|g| g.name.clone())
        .collect();
    live.extend(code_referenced_globals(m));
    let mut queue: Vec<String> = live.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        let targets: Vec<String> = m
            .globals
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.refs.iter().map(|(_, t, _)| t.clone()).collect())
            .unwrap_or_default();
        for t in targets {
            if live.insert(t.clone()) {
                queue.push(t);
            }
        }
    }
    m.globals.retain(|g| !g.internal || live.contains(&g.name));
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
