//! Whole-program validation, dead-global pruning, and dead-home stripping
//! for the PIC8 pipeline: checks what `llvm-link` lets through on the merged
//! module, then drops `internal` globals nothing references so alloc never
//! reserves RAM for them, then drops unread params and pure dead defs so
//! uncalled homes never tile a frame. Expects N translation units already
//! merged into one `.ll` by `llvm-link` (docs/internals/design/31 §7);
//! this stage does not link.

use ir::{collect_global_vals, GepBase, Inst, Module, SrcLoc, Val};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
                used.extend(vals);
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

/// Narrowing of proven-range 16-bit globals (epic-cc#856): a mutable
/// `i16` scalar global whose initializer and every stored value provably
/// fit in a byte becomes `i8`, saving one RAM byte. Loads grow a `zext`
/// to keep every consumer's width; reg stores grow a `trunc`. The proof
/// covers constants, `and` masks, `urem` bounds, narrow `zext`/`trunc`
/// sources, and `phi`/`select` over proven arms. Counters (`add` loops),
/// address-taken globals, volatile accesses, pointer-typed (`ptr_ty`)
/// uses, and mixed-width accesses all stay wide.
pub fn narrow_proven_range(mut m: Module) -> Module {
    let candidates = narrow_candidates(&m);
    if candidates.is_empty() {
        return m;
    }
    let proven = prove_narrow(&m, &candidates);
    if proven.is_empty() {
        return m;
    }
    apply_narrow(&mut m, &proven);
    m
}

/// Per-global use records for the narrowing proof: direct load and store
/// sites plus exclusion flags from every other reference shape.
#[derive(Default)]
struct NarrowUses {
    loads: Vec<(String, usize, usize)>,
    reg_stores: Vec<(String, usize, usize)>,
    const_stores: Vec<(String, usize, usize)>,
    excluded: bool,
}

/// Globals eligible for narrowing before the range proof runs: mutable
/// `i16` scalars with a byte-fitting initializer, no pin, and no
/// relocation, whose every reference is a direct whole-width load or a
/// well-typed store. Each site records `(func, block, inst)` for the
/// rewrite.
fn narrow_candidates(m: &Module) -> Vec<String> {
    let mut uses: std::collections::HashMap<String, NarrowUses> = m
        .globals
        .iter()
        // Only compiler-owned (`internal`) globals narrow: an externally
        // visible global's address is a debugger and harness contract at
        // C type width, and an outside writer can put a full `i16` there
        // no store in this module proves against (epic-cc#856).
        .filter(|g| {
            !g.is_const
                && g.internal
                && g.ty == ir::Ty::I16
                && g.size == 2
                && g.addr.is_none()
                && g.refs.is_empty()
                && g.bytes.len() == 2
                && g.bytes[1] == 0
        })
        .map(|g| (g.name.clone(), NarrowUses::default()))
        .collect();
    if uses.is_empty() {
        return Vec::new();
    }
    let at = |ptr: &str| ptr.strip_prefix('@').map(str::to_owned);
    for f in &m.funcs {
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, inst) in b.insts.iter().enumerate() {
                // Direct whole-width accesses record sites; anything else
                // naming the global excludes it.
                match inst {
                    ir::Inst::Load(l) => {
                        if let Some(g) = at(&l.ptr) {
                            let u = uses.get_mut(&g);
                            match u {
                                Some(u) if l.ty == ir::Ty::I16 && !l.ptr_ty && !l.volatile => {
                                    u.loads.push((f.name.clone(), bi, ii));
                                }
                                Some(u) => u.excluded = true,
                                None => {}
                            }
                        }
                    }
                    ir::Inst::Store(s) => {
                        if let Some(g) = at(&s.ptr) {
                            let u = uses.get_mut(&g);
                            match u {
                                Some(u) if s.ty == ir::Ty::I16 && !s.volatile => match &s.val {
                                    ir::Val::Const(_) => {
                                        u.const_stores.push((f.name.clone(), bi, ii));
                                    }
                                    ir::Val::Reg(_) => {
                                        u.reg_stores.push((f.name.clone(), bi, ii));
                                    }
                                    ir::Val::Global(_) => u.excluded = true,
                                },
                                Some(u) => u.excluded = true,
                                None => {}
                            }
                        }
                        // A stored global address excludes that global.
                        if let ir::Val::Global(g) = &s.val {
                            if let Some(u) = uses.get_mut(g) {
                                u.excluded = true;
                            }
                        }
                    }
                    ir::Inst::Gep(g) => {
                        if let ir::GepBase::Global(g) = &g.base {
                            if let Some(u) = uses.get_mut(g) {
                                u.excluded = true;
                            }
                        }
                    }
                    ir::Inst::Asm(a) => {
                        for op in &a.operands {
                            if let Some(g) = at(&op.ptr) {
                                if let Some(u) = uses.get_mut(&g) {
                                    u.excluded = true;
                                }
                            }
                        }
                        for g in uses.keys().cloned().collect::<Vec<_>>() {
                            if asm_names_global(&a.template, &g) {
                                uses.get_mut(&g).expect("narrow: known candidate").excluded = true;
                            }
                        }
                    }
                    _ => {}
                }
                // Any `Val::Global` operand outside the load/store pointer
                // positions above takes the address or compares it: the
                // global stays wide. BrCond/Switch carry no global by
                // construction but are covered the same way.
                for g in narrow_val_globals(inst) {
                    if let Some(u) = uses.get_mut(&g) {
                        u.excluded = true;
                    }
                }
            }
        }
    }
    // Module-level asm names globals through the same `equ` aliases the
    // backend emits, so a textual reference excludes too.
    for line in &m.module_asm {
        for g in uses.keys().cloned().collect::<Vec<_>>() {
            if asm_names_global(line, &g) {
                uses.get_mut(&g).expect("narrow: known candidate").excluded = true;
            }
        }
    }
    // An initializer reference takes the address too (`static u16 *p =
    // &g` records g in p's refs): a stored pointer can write wide
    // through it after narrowing, so the target stays wide.
    for g in &m.globals {
        for (_, t, _) in &g.refs {
            if let Some(u) = uses.get_mut(t) {
                u.excluded = true;
            }
        }
    }
    // A direct call through a global's address (never produced by the
    // clang shapes irparse accepts, but free to check).
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let ir::Inst::Call(c) = inst {
                    if c.callees.is_empty() {
                        if let Some(u) = uses.get_mut(&c.func) {
                            u.excluded = true;
                        }
                    }
                }
            }
        }
    }
    let mut out: Vec<String> = uses
        .iter()
        // An unreferenced global has no values to prove: shrinking it is
        // dead-code elimination's job (filed separately), and churning
        // the address layout for it only perturbs code factoring.
        .filter(|(_, u)| {
            !u.excluded
                && (!u.loads.is_empty() || !u.reg_stores.is_empty() || !u.const_stores.is_empty())
        })
        .map(|(g, _)| g.clone())
        .collect();
    out.sort();
    out
}

/// Every `Val::Global` operand an instruction carries, in any position.
/// Load/Store pointers are `String`s, handled by the caller.
fn narrow_val_globals(inst: &ir::Inst) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |v: &ir::Val| {
        if let ir::Val::Global(g) = v {
            out.push(g.clone());
        }
    };
    match inst {
        ir::Inst::Store(s) => push(&s.val),
        ir::Inst::Bin(b) => {
            push(&b.a);
            push(&b.b);
        }
        ir::Inst::Ret(Some((_, v)), _) => push(v),
        ir::Inst::Zext(z) => push(&z.val),
        ir::Inst::Sext(x) => push(&x.val),
        ir::Inst::Trunc(t) => push(&t.val),
        ir::Inst::IntToPtr(p) => push(&p.val),
        ir::Inst::Icmp(i) => {
            push(&i.a);
            push(&i.b);
        }
        ir::Inst::Select(s) => {
            push(&s.cond);
            push(&s.a);
            push(&s.b);
        }
        ir::Inst::Call(c) => {
            for arg in &c.args {
                push(&arg.val);
            }
        }
        ir::Inst::BrCond(b) => push(&b.cond),
        ir::Inst::Switch(s) => push(&s.val),
        ir::Inst::Phi(p) => {
            for (v, _) in &p.incoming {
                push(v);
            }
        }
        ir::Inst::Memcpy(mc) => {
            push(&mc.dst);
            push(&mc.src);
            if let ir::MemLen::Reg(v) = &mc.len {
                push(v);
            }
        }
        ir::Inst::Freeze(fr) => push(&fr.val),
        ir::Inst::FloatBin(fb) => {
            push(&fb.a);
            push(&fb.b);
        }
        ir::Inst::Fcmp(fc) => {
            push(&fc.a);
            push(&fc.b);
        }
        ir::Inst::FloatConv(fc) => push(&fc.val),
        _ => {}
    }
    out
}

/// Whether the inline-asm template names global `g`: a token equal to
/// `g` or its `_`-prefixed `equ` alias. Tokens keep `.` so dotted
/// link-renames (`g_s.2`) compare whole.
fn asm_names_global(template: &str, g: &str) -> bool {
    let aliased = format!("_{g}");
    template
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '$'))
        .any(|tok| tok == g || tok == aliased)
}

/// The defining instruction's destination name, if it defines a value.
/// Calls count (their results are unprovable, but they occupy the name).
fn inst_dst(inst: &ir::Inst) -> Option<&str> {
    match inst {
        ir::Inst::Load(l) => Some(&l.dst),
        ir::Inst::Bin(b) => Some(&b.dst),
        ir::Inst::Zext(z) => Some(&z.dst),
        ir::Inst::Sext(x) => Some(&x.dst),
        ir::Inst::Trunc(t) => Some(&t.dst),
        ir::Inst::IntToPtr(p) => Some(&p.dst),
        ir::Inst::Icmp(i) => Some(&i.dst),
        ir::Inst::Select(s) => Some(&s.dst),
        ir::Inst::Call(c) => c.dst.as_deref(),
        ir::Inst::Phi(p) => Some(&p.dst),
        ir::Inst::Gep(g) => Some(&g.dst),
        ir::Inst::Alloca(a) => Some(&a.dst),
        ir::Inst::VaArg(v) => Some(&v.dst),
        ir::Inst::FloatBin(f) => Some(&f.dst),
        ir::Inst::Fcmp(f) => Some(&f.dst),
        ir::Inst::FloatConv(f) => Some(&f.dst),
        _ => None,
    }
}

/// The narrowing proof context: definition index over the module plus
/// the current fixpoint set. Params count as defined-but-unprovable;
/// ambiguous names (two definitions, never valid SSA) never prove.
struct Prover<'m> {
    m: &'m Module,
    proven: std::collections::HashSet<String>,
    defs: std::collections::HashMap<(String, String), &'m ir::Inst>,
    ambiguous: std::collections::HashSet<(String, String)>,
    params: std::collections::HashSet<(String, String)>,
}

/// The least fixpoint of proven-narrow globals: a candidate is proven
/// when every stored value fits in a byte under the current set. Loads
/// of proven globals count as fitting, so narrowing flows through
/// globals that only copy each other.
fn prove_narrow(m: &Module, candidates: &[String]) -> std::collections::HashSet<String> {
    let mut defs = std::collections::HashMap::new();
    let mut ambiguous = std::collections::HashSet::new();
    let mut params = std::collections::HashSet::new();
    for f in &m.funcs {
        for p in &f.params {
            params.insert((f.name.clone(), p.name.clone()));
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Some(dst) = inst_dst(inst) {
                    let key = (f.name.clone(), dst.to_string());
                    if defs.contains_key(&key) || params.contains(&key) {
                        ambiguous.insert(key);
                    } else {
                        defs.insert(key, inst);
                    }
                }
            }
        }
    }
    // A load whose destination is ambiguously defined would rename the
    // wrong uses on rewrite, so its global never proves.
    let set: std::collections::HashSet<&str> = candidates.iter().map(String::as_str).collect();
    let mut blocked: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let ir::Inst::Load(l) = inst {
                    if let Some(g) = l.ptr.strip_prefix('@') {
                        if set.contains(g) && ambiguous.contains(&(f.name.clone(), l.dst.clone())) {
                            blocked.insert(g.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut prover = Prover {
        m,
        proven: std::collections::HashSet::new(),
        defs,
        ambiguous,
        params,
    };
    loop {
        let mut grew = false;
        for g in candidates {
            if blocked.contains(g) || prover.proven.contains(g) {
                continue;
            }
            if prover.global_proven(g) {
                prover.proven.insert(g.clone());
                grew = true;
            }
        }
        if !grew {
            return prover.proven;
        }
    }
}

/// Whether the 16-bit pattern `k` has a zero high byte. LLVM constants
/// are canonical, so this is exactly the stored high byte.
fn const_fits_byte(k: i64) -> bool {
    (k as u16) <= 0xFF
}

impl Prover<'_> {
    /// Whether every value stored to global `g` fits in a byte:
    /// constants in range plus registers whose definitions prove out.
    fn global_proven(&self, g: &str) -> bool {
        for f in &self.m.funcs {
            for b in &f.blocks {
                for inst in &b.insts {
                    if let ir::Inst::Store(s) = inst {
                        if s.ptr.strip_prefix('@') == Some(g) {
                            let ok = match &s.val {
                                ir::Val::Const(k) => const_fits_byte(*k),
                                ir::Val::Reg(r) => {
                                    let mut visiting = std::collections::HashSet::new();
                                    self.reg_fits(&f.name, r, &mut visiting)
                                }
                                ir::Val::Global(_) => false,
                            };
                            if !ok {
                                return false;
                            }
                        }
                    }
                }
            }
        }
        true
    }

    /// Whether the value `%r` in `func` provably holds a zero high byte:
    /// constants, narrow sources, masking/bounding ops, and
    /// `phi`/`select` over fitting arms. Pointer-flavored defs, calls,
    /// and ambiguous or missing definitions never fit. `visiting`
    /// breaks `phi`-backedge cycles as unproven.
    fn reg_fits(
        &self,
        func: &str,
        r: &str,
        visiting: &mut std::collections::HashSet<(String, String)>,
    ) -> bool {
        if !visiting.insert((func.to_string(), r.to_string())) {
            return false;
        }
        let fits = self.reg_fits_inner(func, r, visiting);
        visiting.remove(&(func.to_string(), r.to_string()));
        fits
    }

    fn reg_fits_inner(
        &self,
        func: &str,
        r: &str,
        visiting: &mut std::collections::HashSet<(String, String)>,
    ) -> bool {
        let key = (func.to_string(), r.to_string());
        if self.params.contains(&key) || self.ambiguous.contains(&key) {
            return false;
        }
        let Some(def) = self.defs.get(&key).copied() else {
            return false;
        };
        match def {
            ir::Inst::Load(l) => {
                if l.volatile || l.ptr_ty {
                    return false;
                }
                // A byte read is already a byte; a wide global load
                // borrows the fixpoint.
                if l.ty.bytes() <= 1 {
                    return true;
                }
                l.ptr
                    .strip_prefix('@')
                    .is_some_and(|g| self.proven.contains(g))
            }
            ir::Inst::Bin(b) => match (&b.op, &b.a, &b.b) {
                // Each `and` bit clears when either side clears it: one
                // fitting side zeroes the high byte.
                (ir::BinOp::And, a, b) => {
                    self.val_fits(func, a, visiting) || self.val_fits(func, b, visiting)
                }
                // `or`/`xor` set high bits from either side: both must fit.
                (ir::BinOp::Or | ir::BinOp::Xor, a, b) => {
                    self.val_fits(func, a, visiting) && self.val_fits(func, b, visiting)
                }
                // Unsigned remainder is below the divisor; divisor 0 is
                // UB and proves nothing.
                (ir::BinOp::URem, _, ir::Val::Const(d)) => (*d as u16) >= 1 && (*d as u16) <= 256,
                _ => false,
            },
            // Zero extension writes the high byte itself.
            ir::Inst::Zext(z) => z.from.bytes() <= 1 || self.val_fits(func, &z.val, visiting),
            // A narrowed result is already a byte.
            ir::Inst::Trunc(t) => t.to.bytes() <= 1,
            ir::Inst::Select(s) => {
                self.val_fits(func, &s.a, visiting) && self.val_fits(func, &s.b, visiting)
            }
            ir::Inst::Phi(p) => {
                !p.ptr
                    && p.incoming
                        .iter()
                        .all(|(v, _)| self.val_fits(func, v, visiting))
            }
            ir::Inst::Freeze(fr) => self.val_fits(func, &fr.val, visiting),
            _ => false,
        }
    }

    fn val_fits(
        &self,
        func: &str,
        v: &ir::Val,
        visiting: &mut std::collections::HashSet<(String, String)>,
    ) -> bool {
        match v {
            ir::Val::Const(k) => const_fits_byte(*k),
            ir::Val::Reg(q) => self.reg_fits(func, q, visiting),
            ir::Val::Global(_) => false,
        }
    }
}

/// Rewrite every proven global: `i16` becomes `i8` with its low init
/// byte, loads grow a widening `zext`, and reg stores grow a `trunc`.
/// Consumers keep their widths behind the new joints, so no other
/// instruction changes.
fn apply_narrow(m: &mut Module, proven: &std::collections::HashSet<String>) {
    for g in &mut m.globals {
        if proven.contains(&g.name) {
            g.ty = ir::Ty::I8;
            g.size = 1;
            g.bytes = vec![g.bytes[0]];
        }
    }
    for f in &mut m.funcs {
        // Fresh-name pool: every SSA name plus every name this pass
        // mints, so joints never collide with user values.
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in &f.params {
            taken.insert(p.name.clone());
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Some(dst) = inst_dst(inst) {
                    taken.insert(dst.to_string());
                }
            }
        }
        let mut fresh = |base: &str| -> String {
            let mut n = format!("{base}.narrow");
            let mut i = 1;
            while taken.contains(&n) {
                i += 1;
                n = format!("{base}.narrow.{i}");
            }
            taken.insert(n.clone());
            n
        };
        // Joints first, renames after: a store's trunc must see the
        // renamed value when it consumes a narrowed load, so every
        // joint lands before any rename runs.
        let mut renames: Vec<(String, String)> = Vec::new();
        for b in &mut f.blocks {
            let mut out: Vec<ir::Inst> = Vec::with_capacity(b.insts.len() + 2);
            for inst in b.insts.drain(..) {
                match inst {
                    ir::Inst::Load(mut l)
                        if l.ptr.strip_prefix('@').is_some_and(|g| proven.contains(g)) =>
                    {
                        l.ty = ir::Ty::I8;
                        let old = l.dst.clone();
                        let new = fresh(&old);
                        out.push(ir::Inst::Load(l));
                        out.push(ir::Inst::Zext(ir::Zext {
                            dst: new.clone(),
                            from: ir::Ty::I8,
                            val: ir::Val::Reg(old.clone()),
                            to: ir::Ty::I16,
                            loc: None,
                        }));
                        renames.push((old, new));
                    }
                    ir::Inst::Store(mut s)
                        if s.ptr.strip_prefix('@').is_some_and(|g| proven.contains(g)) =>
                    {
                        match s.val {
                            ir::Val::Const(_) => {
                                s.ty = ir::Ty::I8;
                                out.push(ir::Inst::Store(s));
                            }
                            ir::Val::Reg(v) => {
                                let t = fresh(&format!("{v}.tr"));
                                out.push(ir::Inst::Trunc(ir::Trunc {
                                    dst: t.clone(),
                                    from: ir::Ty::I16,
                                    val: ir::Val::Reg(v),
                                    to: ir::Ty::I8,
                                    loc: None,
                                }));
                                s.ty = ir::Ty::I8;
                                s.val = ir::Val::Reg(t);
                                out.push(ir::Inst::Store(s));
                            }
                            ir::Val::Global(_) => {
                                panic!("wholeprog: narrowed store of a global address")
                            }
                        }
                    }
                    other => out.push(other),
                }
            }
            b.insts = out;
        }
        for (old, new) in &renames {
            rename_reg_uses(f, old, new);
        }
    }
    // A narrowed global must have no wide access left: the scans above
    // admit only whole-width direct loads and stores, and the rewrite
    // visits every recorded site.
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    ir::Inst::Load(l)
                        if l.ptr.strip_prefix('@').is_some_and(|g| proven.contains(g)) =>
                    {
                        assert!(
                            l.ty == ir::Ty::I8,
                            "wholeprog: narrowed load of {} stayed wide",
                            l.ptr
                        );
                    }
                    ir::Inst::Store(s)
                        if s.ptr.strip_prefix('@').is_some_and(|g| proven.contains(g)) =>
                    {
                        assert!(
                            s.ty == ir::Ty::I8,
                            "wholeprog: narrowed store to {} stayed wide",
                            s.ptr
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Replace every `Reg(old)` operand in `f` with `Reg(new)`, except the
/// widening `zext` that still reads `old`. Callers insert the joint
/// before renaming, so skipping exactly that instruction is sound.
fn rename_reg_uses(f: &mut ir::Func, old: &str, new: &str) {
    let sub = |v: &mut ir::Val| {
        if matches!(v, ir::Val::Reg(r) if r == old) {
            *v = ir::Val::Reg(new.to_string());
        }
    };
    for b in &mut f.blocks {
        for inst in &mut b.insts {
            // The joint itself keeps reading the narrow load.
            if let ir::Inst::Zext(z) = inst {
                if z.dst == new {
                    continue;
                }
            }
            match inst {
                ir::Inst::Store(s) => sub(&mut s.val),
                ir::Inst::Bin(bin) => {
                    sub(&mut bin.a);
                    sub(&mut bin.b);
                }
                ir::Inst::Ret(Some((_, v)), _) => sub(v),
                ir::Inst::Zext(z) => sub(&mut z.val),
                ir::Inst::Sext(x) => sub(&mut x.val),
                ir::Inst::Trunc(t) => sub(&mut t.val),
                ir::Inst::IntToPtr(p) => sub(&mut p.val),
                ir::Inst::Icmp(i) => {
                    sub(&mut i.a);
                    sub(&mut i.b);
                }
                ir::Inst::Select(s) => {
                    sub(&mut s.cond);
                    sub(&mut s.a);
                    sub(&mut s.b);
                }
                ir::Inst::Call(c) => {
                    for arg in &mut c.args {
                        sub(&mut arg.val);
                    }
                }
                ir::Inst::BrCond(b) => sub(&mut b.cond),
                ir::Inst::Switch(s) => sub(&mut s.val),
                ir::Inst::Gep(g) => {
                    // Dynamic index terms read the value: a narrowed
                    // load's term must name the widening joint, or the
                    // backend sees an i8 reg where it expects two bytes.
                    for (_, r) in &mut g.terms {
                        if r == old {
                            *r = new.to_string();
                        }
                    }
                }
                ir::Inst::Phi(p) => {
                    for (v, _) in &mut p.incoming {
                        sub(v);
                    }
                }
                ir::Inst::Memcpy(mc) => {
                    sub(&mut mc.dst);
                    sub(&mut mc.src);
                    if let ir::MemLen::Reg(v) = &mut mc.len {
                        sub(v);
                    }
                }
                ir::Inst::Freeze(fr) => sub(&mut fr.val),
                ir::Inst::FloatBin(fb) => {
                    sub(&mut fb.a);
                    sub(&mut fb.b);
                }
                ir::Inst::Fcmp(fc) => {
                    sub(&mut fc.a);
                    sub(&mut fc.b);
                }
                ir::Inst::FloatConv(fc) => sub(&mut fc.val),
                _ => {}
            }
        }
    }
}

/// Drops homes the backends never read (epic-cc#1338): unread callee
/// params with their direct-site args, and pure defs with no uses.
/// Fewer param slots lets a single-site callee dip into its caller
/// frame, and fewer temp slots lowers the frame high-water mark.
pub fn strip_dead(mut m: Module) -> Module {
    let taken = address_taken(&m);
    loop {
        let params = strip_dead_params(&mut m, &taken);
        let defs = strip_dead_defs(&mut m);
        if !params && !defs {
            return m;
        }
    }
}

/// Functions no indirect call can reach: every `Val::Global` naming a
/// defined function (the `fill_indirect_callees` set), plus const refs
/// and `module_asm` mentions. Computed once: later sweeps erase dead
/// mentions, and a shrinking set would unprotect a taken function.
fn address_taken(m: &Module) -> HashSet<String> {
    let defined: HashSet<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    let mut taken: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                let mut vals = HashSet::new();
                ir::collect_global_vals(inst, &mut vals);
                taken.extend(vals.into_iter().filter(|v| defined.contains(v.as_str())));
                // Indirect candidates legalize resolved (ISR `_isr` copies
                // included): no `Val::Global` names them after the store
                // rewrite, so the candidate lists protect them instead.
                if let Inst::Call(c) = inst {
                    taken.extend(
                        c.callees
                            .iter()
                            .filter(|v| defined.contains(v.as_str()))
                            .cloned(),
                    );
                }
            }
        }
    }
    for g in &m.globals {
        for (_, target, _) in &g.refs {
            if defined.contains(target.as_str()) {
                taken.insert(target.clone());
            }
        }
    }
    let mut asm_taken = BTreeSet::new();
    for entry in &m.module_asm {
        mark_text_refs(&defined, &mut asm_taken, entry);
    }
    taken.extend(asm_taken);
    taken
}

/// One dead-param sweep against a frozen taken set. A param strips when
/// the body never reads it and the function has a body, is not
/// `main`/ISR/naked, is not a runtime routine, and is not address-taken.
/// Direct sites rewrite positionally; byval/sret params stay, and a site
/// whose arity mismatches pins the function for this sweep.
fn strip_dead_params(m: &mut Module, taken: &HashSet<String>) -> bool {
    let mut changed = false;
    for fi in 0..m.funcs.len() {
        let (fname, named, variadic, dead) = {
            let f = &m.funcs[fi];
            if f.name == "main"
                || f.isr
                || f.naked
                || f.blocks.is_empty()
                || f.unreachable_body
                || ir::is_runtime_routine(&f.name)
                || taken.contains(&f.name)
            {
                continue;
            }
            let mut reads = HashSet::new();
            for b in &f.blocks {
                for inst in &b.insts {
                    reads.extend(ir::read_vals(inst));
                }
            }
            let dead: Vec<usize> = f
                .params
                .iter()
                .enumerate()
                .filter(|(_, p)| p.byval.is_none() && !p.sret && !reads.contains(&p.name))
                .map(|(i, _)| i)
                .collect();
            if dead.is_empty() {
                continue;
            }
            (f.name.clone(), f.params.len(), f.variadic, dead)
        };
        let pinned = m.funcs.iter().any(|g| {
            g.blocks.iter().any(|b| {
                b.insts.iter().any(|inst| match inst {
                    Inst::Call(c) if c.callees.is_empty() && c.func == fname => {
                        c.args.len() != named && !(variadic && c.args.len() > named)
                    }
                    _ => false,
                })
            })
        });
        if pinned {
            continue;
        }
        m.funcs[fi].params = m.funcs[fi]
            .params
            .drain(..)
            .enumerate()
            .filter(|(i, _)| !dead.contains(i))
            .map(|(_, p)| p)
            .collect();
        for g in &mut m.funcs {
            for b in &mut g.blocks {
                for inst in &mut b.insts {
                    if let Inst::Call(c) = inst {
                        if c.callees.is_empty() && c.func == fname {
                            for &i in dead.iter().rev() {
                                if i < c.args.len() {
                                    c.args.remove(i);
                                }
                            }
                        }
                    }
                }
            }
        }
        changed = true;
    }
    changed
}

/// One dead-def sweep. A def strips when nothing reads it and it is
/// pure ALU (`Bin`, `Trunc`, `Zext`, `Sext`, `Icmp`, `Select`,
/// `Freeze`): no memory, control, or flag reader observes the missing
/// write. Loads stay (banking pairs with their emission), as do calls,
/// phis, allocas, and pointer makers.
fn strip_dead_defs(m: &mut Module) -> bool {
    let mut changed = false;
    for f in &mut m.funcs {
        loop {
            let mut uses: HashMap<String, usize> = HashMap::new();
            for b in &f.blocks {
                for inst in &b.insts {
                    for v in ir::read_vals(inst) {
                        *uses.entry(v).or_default() += 1;
                    }
                }
            }
            let mut struck = false;
            for b in &mut f.blocks {
                let before = b.insts.len();
                b.insts.retain(|inst| {
                    let dst = match inst {
                        Inst::Bin(q) => Some(&q.dst),
                        Inst::Trunc(t) => Some(&t.dst),
                        Inst::Zext(z) => Some(&z.dst),
                        Inst::Sext(x) => Some(&x.dst),
                        Inst::Icmp(c) => Some(&c.dst),
                        Inst::Select(s) => Some(&s.dst),
                        Inst::Freeze(fr) => Some(&fr.dst),
                        _ => None,
                    };
                    !dst.is_some_and(|d| uses.get(d).is_none_or(|&n| n == 0))
                });
                struck |= b.insts.len() != before;
            }
            if !struck {
                break;
            }
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse text IR and give every named global a zero initializer, the
    /// shape irparse produces for zeroed globals. Test globals are
    /// compiler-owned (`internal`): the pass never narrows externally
    /// visible ones.
    fn zeroed(text: &str) -> Module {
        let mut m = ir::parse(text);
        for g in &mut m.globals {
            if !g.is_const {
                g.bytes = vec![0; g.size as usize];
                g.internal = true;
            }
        }
        m
    }

    fn narrowed_ty(m: &Module, name: &str) -> ir::Ty {
        m.globals
            .iter()
            .find(|g| g.name == name)
            .expect("test global exists")
            .ty
    }

    fn loads_of<'m>(m: &'m Module, name: &str) -> Vec<&'m ir::Load> {
        m.funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                ir::Inst::Load(l) if l.ptr == format!("@{name}") => Some(l),
                _ => None,
            })
            .collect()
    }

    /// Constant-only stores narrow: the global shrinks, loads widen
    /// behind a `zext`, and consumers keep reading the wide name.
    #[test]
    fn const_only_stores_narrow() {
        let m = zeroed(
            "global g i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             store i16 2 @g\n\
             %1 = load i16 @g\n\
             %2 = add i16 %1 1\n\
             ret i16 %2\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I8);
        let g = m.globals.iter().find(|g| g.name == "g").unwrap();
        assert_eq!((g.size, g.bytes.as_slice()), (1, [0].as_slice()));
        let loads = loads_of(&m, "g");
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].ty, ir::Ty::I8);
        let zexts: Vec<_> = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                ir::Inst::Zext(z) if z.from == ir::Ty::I8 && z.to == ir::Ty::I16 => Some(z),
                _ => None,
            })
            .collect();
        assert_eq!(zexts.len(), 1);
        // The consumer still reads the wide joint, not the narrow load.
        assert!(matches!(&zexts[0].val, ir::Val::Reg(r) if r == "1"));
        let adds: Vec<_> = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                ir::Inst::Bin(b) => Some(b),
                _ => None,
            })
            .collect();
        assert_eq!(adds.len(), 1);
        assert!(matches!(&adds[0].a, ir::Val::Reg(r) if r == &zexts[0].dst));
    }

    /// A `urem` bound below 256 narrows even when the dividend is wild.
    #[test]
    fn urem_bound_narrows() {
        let m = zeroed(
            "global g i16\n\
             global other i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             %1 = load i16 @other\n\
             %2 = urem i16 %1 3\n\
             store i16 %2 @g\n\
             %3 = load i16 @g\n\
             ret i16 %3\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I8);
        // The trunc joint carries the stored value into the narrow store.
        let truncs: Vec<_> = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                ir::Inst::Trunc(t) if t.from == ir::Ty::I16 && t.to == ir::Ty::I8 => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(truncs.len(), 1);
        assert!(matches!(&truncs[0].val, ir::Val::Reg(r) if r == "2"));
    }

    /// A `phi` over fitting arms narrows; the counter loop stays wide.
    #[test]
    fn phi_consts_narrow_but_counter_does_not() {
        let m = zeroed(
            "global mode i16\n\
             global ticks i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             %t = load i16 @ticks\n\
             %t2 = add i16 %t 1\n\
             store i16 %t2 @ticks\n\
             %c = icmp eq i8 0 0\n\
             br i1 %c 1 2\n\
             block 1:\n\
             br 3\n\
             block 2:\n\
             br 3\n\
             block 3:\n\
             %v = phi i16 0 1 2 2\n\
             store i16 %v @mode\n\
             ret i16 %v\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "mode"), ir::Ty::I8);
        assert_eq!(narrowed_ty(&m, "ticks"), ir::Ty::I16);
    }

    /// A volatile load marks ISR-shared state: the global stays wide.
    #[test]
    fn volatile_access_stays_wide() {
        let m = zeroed(
            "global flag i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             store i16 1 @flag\n\
             %1 = load volatile i16 @flag\n\
             ret i16 %1\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "flag"), ir::Ty::I16);
    }

    /// Storing the global's address takes it: the global stays wide.
    #[test]
    fn address_taken_stays_wide() {
        let m = zeroed(
            "global g i16\n\
             global slot i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             store i16 1 @g\n\
             store i16 @g @slot\n\
             ret i16 0\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I16);
    }

    /// An initializer reference takes the address the same way a stored
    /// one does (`static u16 *p = &g`): the target stays wide while an
    /// unreferenced sibling still narrows.
    #[test]
    fn initializer_ref_stays_wide() {
        let mut m = zeroed(
            "global g i16\n\
             global p i16\n\
             global q i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             store i16 1 @g\n\
             store i16 2 @q\n\
             ret i16 0\n",
        );
        m.globals
            .iter_mut()
            .find(|g| g.name == "p")
            .expect("p parses")
            .refs
            .push((0, "g".to_string(), 0));
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I16);
        assert_eq!(narrowed_ty(&m, "q"), ir::Ty::I8);
    }

    /// A wide `urem` divisor proves nothing: the global stays wide.
    #[test]
    fn wide_urem_bound_stays_wide() {
        let m = zeroed(
            "global g i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             %1 = load i16 @g\n\
             %2 = urem i16 %1 1000\n\
             store i16 %2 @g\n\
             ret i16 %2\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I16);
    }

    /// An externally visible global stays wide even when every in-module
    /// store fits: an outside writer (harness poke, debugger, linked
    /// writer) can put a full `i16` there. This is the pid_clamp shape:
    #[test]
    fn external_linkage_stays_wide() {
        let mut m = zeroed(
            "global in_a i16\n\
             fn main(i16) ()\n\
             block 0:\n\
             %1 = load i16 @in_a\n\
             %2 = add i16 %1 1\n\
             ret i16 %2\n",
        );
        m.globals
            .iter_mut()
            .find(|g| g.name == "in_a")
            .expect("test global exists")
            .internal = false;
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "in_a"), ir::Ty::I16);
    }

    /// A narrowed load feeding a GEP dynamic term renames the term to
    /// the widening joint: the backend sizes index terms by reg width,
    /// so the old narrow name would trip its wide-term assert.
    #[test]
    fn gep_index_term_names_the_widening_joint() {
        let m = zeroed(
            "global g i16\n\
             const t i8\n\
             fn main(i8) ()\n\
             block 0:\n\
             store i16 3 @g\n\
             %1 = load i16 @g\n\
             %p = gep @t +0 +1*%1\n\
             %v = load i8 %p\n\
             ret i8 %v\n",
        );
        let m = narrow_proven_range(m);
        assert_eq!(narrowed_ty(&m, "g"), ir::Ty::I8);
        let zext_dst: String = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                ir::Inst::Zext(z) => Some(z.dst.clone()),
                _ => None,
            })
            .expect("narrowed load grows a zext");
        let terms: Vec<String> = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                ir::Inst::Gep(g) => {
                    Some(g.terms.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(terms, vec![zext_dst]);
    }

    /// Unread params drop out of the signature and their args drop out
    /// of every direct site: the memcpy shape (three dead params, one
    /// direct caller) loses all six homes.
    #[test]
    fn dead_params_and_args_strip() {
        let m = ir::parse(
            "fn callee(void) (0=i16, 1=ptr, 2=i8)\n\
             block 0:\n\
             ret void\n\
             fn main(void) ()\n\
             block 0:\n\
             call void @callee(i16 32, @g, i8 1)\n\
             ret void\n",
        );
        let m = strip_dead(m);
        let callee = m.funcs.iter().find(|f| f.name == "callee").unwrap();
        assert!(callee.params.is_empty());
        let site = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                ir::Inst::Call(c) if c.func == "callee" => Some(c),
                _ => None,
            })
            .expect("direct site survives");
        assert!(site.args.is_empty());
    }

    /// A read param keeps its slot while a dead neighbor strips: the
    /// surviving arg stays aligned with the surviving param.
    #[test]
    fn live_param_keeps_args_aligned() {
        let m = ir::parse(
            "fn callee(void) (0=i16, 1=i16)\n\
             block 0:\n\
             store volatile i16 %0 @g\n\
             ret void\n\
             fn main(void) ()\n\
             block 0:\n\
             call void @callee(i16 7, i16 9)\n\
             ret void\n",
        );
        let m = strip_dead(m);
        let callee = m.funcs.iter().find(|f| f.name == "callee").unwrap();
        assert_eq!(callee.params.len(), 1);
        assert_eq!(callee.params[0].name, "0");
        let site = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                ir::Inst::Call(c) if c.func == "callee" => Some(c),
                _ => None,
            })
            .expect("direct site survives");
        assert_eq!(site.args.len(), 1);
        assert!(matches!(&site.args[0].val, ir::Val::Const(7)));
    }

    /// An address-taken function keeps unread params: an indirect site
    /// still passes the full arity, so the signature must not narrow.
    #[test]
    fn address_taken_keeps_dead_params() {
        let m = ir::parse(
            "fn cb(void) (0=i16)\n\
             block 0:\n\
             ret void\n\
             fn sink(void) (p=ptr)\n\
             block 0:\n\
             ret void\n\
             fn main(void) ()\n\
             block 0:\n\
             call void @sink(@cb)\n\
             ret void\n",
        );
        let m = strip_dead(m);
        let cb = m.funcs.iter().find(|f| f.name == "cb").unwrap();
        assert_eq!(cb.params.len(), 1);
    }

    /// An indirect candidate keeps unread params: the site passes full
    /// arity, and only the candidate list names the copy once the store
    /// rewrite leaves the original spelling behind.
    #[test]
    fn indirect_callee_keeps_dead_params() {
        let mut m = ir::parse(
            "fn f_isr(void) (0=i16, 1=i16)\n\
             block 0:\n\
             ret void\n\
             fn main(void) ()\n\
             block 0:\n\
             call void @f_isr(i16 7, i16 9)\n\
             ret void\n",
        );
        for f in &mut m.funcs {
            for b in &mut f.blocks {
                for inst in &mut b.insts {
                    if let ir::Inst::Call(c) = inst {
                        c.func = "0".to_string();
                        c.callees = vec!["f_isr".to_string()];
                    }
                }
            }
        }
        let m = strip_dead(m);
        let copy = m.funcs.iter().find(|f| f.name == "f_isr").unwrap();
        assert_eq!(copy.params.len(), 2);
        let site = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                ir::Inst::Call(c) => Some(c),
                _ => None,
            })
            .expect("indirect site survives");
        assert_eq!(site.args.len(), 2);
    }

    /// Pure defs with no uses strip and orphan their operands: the dead
    /// chain collapses, while the fused loop latch compare stays (the
    /// branch still reads it) along with the param it reads.
    #[test]
    fn dead_defs_cascade_and_fused_latch_stays() {
        let m = ir::parse(
            "global g i16\n\
             fn f(void) (0=i16)\n\
             block 0:\n\
             %1 = add i16 %0 1\n\
             %2 = add i16 %1 2\n\
             %3 = icmp eq i16 %0 32\n\
             br i1 %3 1 1\n\
             block 1:\n\
             store volatile i16 %0 @g\n\
             ret void\n\
             fn main(void) ()\n\
             block 0:\n\
             ret void\n",
        );
        let m = strip_dead(m);
        let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
        let bins: Vec<_> = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, ir::Inst::Bin(_)))
            .collect();
        assert!(bins.is_empty());
        assert!(f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(
            i,
            ir::Inst::Icmp(c) if c.dst == "3"
        )));
        assert_eq!(f.params.len(), 1);
    }
}
