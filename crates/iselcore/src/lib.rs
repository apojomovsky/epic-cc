//! `iselcore` — shared instruction-selection primitives used by both the
//! PIC14 (`isel`) and PIC18 (`isel-pic18`) backends.

use ir::{GepBase, Inst, Module};
use std::collections::HashMap;

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
pub fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// Where a local's bytes live. Only `Direct` is constructed: the `Frame`
/// case stays reserved (docs/29 §2) so frame-pointer handling for
/// recursion and reentrancy adds a case here and at `Slot`
/// construction sites without touching address-resolution call sites.
pub enum Slot {
    /// Statically allocated: a direct file address.
    Direct(u16),
    /// Frame-relative, FSR2 + offset. Reserved for reentrant
    /// handling; nothing constructs this.
    #[allow(dead_code)]
    Frame(i8),
}

impl Slot {
    /// Only `Direct` is constructed.
    pub fn direct(&self) -> u16 {
        match self {
            Slot::Direct(a) => *a,
            Slot::Frame(_) => {
                unimplemented!("frame-relative slots arrive with the reentrancy phase")
            }
        }
    }
}

/// Parse alloc address-map text into `HashMap<String, u16>`.
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become entries
/// (locals keyed `{func}::{name}`); `const <name>` lines name flash globals
/// with no RAM address and skip: isel reads their bytes from the `Module`.
/// Both backends share this parser over alloc output: nothing in it is
/// PIC14-specific.
pub fn parse_map(text: &str) -> HashMap<String, u16> {
    let mut addrs = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().expect("map entry");
        match kw {
            "const" => {
                // Flash global: no RAM address; nothing to record.
            }
            "global" => {
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("iselcore: bad address in map line: {line}"));
                addrs.insert(name, addr);
            }
            "local" => {
                let func = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("iselcore: bad address in map line: {line}"));
                addrs.insert(format!("{func}::{name}"), addr);
            }
            _ => panic!("iselcore: unexpected map line: {line}"),
        }
    }
    addrs
}

/// Where a pointer's bytes ultimately live, once every `gep` in its chain
/// has been folded away: a named global, or a local slot (an alloca's own
/// buffer, or a byval/sret param, where the `bool` is `true` for sret, meaning
/// the slot holds a target ADDRESS rather than being the object itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    Global(String),
    Slot(String, bool),
}

/// Resolved pointer chains: `(base, constant offset, dynamic terms)` per SSA key.
/// Offsets are `u16`: globals can exceed the 255-byte stack-slot ceiling.
pub type PtrResolution = HashMap<String, (Base, u16, Vec<(u16, String)>)>;

/// Fold every `Gep` and pointer-typed `Select` in `m` to `(base, k, terms)`.
/// `base` starts the chain, `k` adds the constant offset, `terms` appends
/// dynamic offsets inner-first, keyed `{func}::{reg}` via `ssa_key`.
/// Seeds (byval/sret params, allocas, runtime pointer values) anchor the
/// fixpoint; pointer selects fold when both arms share a base and term set,
/// else seed as indirect slots holding address bytes isel materializes.
/// A base that is neither a seed nor a pending entry breaks an earlier-stage
/// invariant and panics, as do unmaterializable arms and stalled scans.
pub fn resolve_pointers(m: &Module) -> PtrResolution {
    let mut geps: HashMap<String, ir::Gep> = HashMap::new();
    let mut selects: HashMap<String, ir::Select> = HashMap::new();
    let mut resolved: PtrResolution = HashMap::new();
    for f in &m.funcs {
        for p in &f.params {
            if p.byval.is_some() {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), false), 0, Vec::new()),
                );
            } else if p.sret {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), true), 0, Vec::new()),
                );
            } else if p.ptr {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), false), 0, Vec::new()),
                );
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Gep(g) => {
                        geps.insert(ssa_key(&f.name, &g.dst), g.clone());
                    }
                    Inst::Alloca(a) => {
                        resolved.insert(
                            ssa_key(&f.name, &a.dst),
                            (Base::Slot(a.dst.clone(), false), 0, Vec::new()),
                        );
                    }
                    Inst::Select(s) if s.ptr => {
                        selects.insert(ssa_key(&f.name, &s.dst), s.clone());
                    }
                    Inst::VaArg(v) if v.ptr_ty => {
                        // A `va_arg ptr` result is a runtime pointer VALUE:
                        // its two bytes live in the dst slot (a GEP over it
                        // or a deref through it reads those bytes), the same
                        // model as IntToPtr and the load-ptr seed
                        // (epic-cc#131).
                        resolved.insert(
                            ssa_key(&f.name, &v.dst),
                            (Base::Slot(v.dst.clone(), true), 0, Vec::new()),
                        );
                    }
                    Inst::IntToPtr(p) => {
                        // A runtime integer address becoming a pointer VALUE:
                        // the dst slot holds the two address bytes, so every
                        // load/store through it lowers as an indirect
                        // (sret-style) FSR/INDF access. The address bytes are
                        // materialized by isel's `Inst::IntToPtr` lowering.
                        resolved.insert(
                            ssa_key(&f.name, &p.dst),
                            (Base::Slot(p.dst.clone(), true), 0, Vec::new()),
                        );
                    }
                    Inst::Load(l) if l.ptr_ty => {
                        // A `load ptr` result is a runtime pointer VALUE:
                        // the dst slot holds the two loaded address bytes
                        // (a GEP over it, a deref through it, or a ptr arg
                        // pass all read those bytes). Seeding it as an
                        // indirect slot lets a GEP chain over a loaded
                        // handle field resolve (the HAL's
                        // `h->OverflowCallback` shape, epic-cc#183).
                        resolved.insert(
                            ssa_key(&f.name, &l.dst),
                            (Base::Slot(l.dst.clone(), true), 0, Vec::new()),
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    // Pointer-typed selects whose two arms are compile-time pointer
    // constants (the literal `inttoptr (<ty> <k> to ptr)` arms clang emits
    // for the HAL's `pir_reg_addr(d)`) are runtime address VALUES: the
    // selected arm's address bytes land in the dst slot (isel materializes
    // the select as a 2-byte value select), so the dst dereferences as an
    // indirect slot like an IntToPtr result.
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut progressed = true;
        while progressed {
            progressed = false;
            for (key, s) in selects
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("{fname}::")))
            {
                if resolved.contains_key(key) {
                    continue;
                }
                let const_arm = |v: &ir::Val| matches!(v, ir::Val::Const(_));
                if const_arm(&s.a) && const_arm(&s.b) {
                    resolved.insert(
                        key.clone(),
                        (Base::Slot(s.dst.clone(), true), 0, Vec::new()),
                    );
                    progressed = true;
                }
            }
        }
    }
    /// Try seeding one pointer-typed phi (`phi ptr [...]`) as an indirect
    /// slot: succeeds when every incoming is a runtime-address value.
    /// Accepted arms: `Const`/`Global` link-time addresses; regs already
    /// seeded as address slots (indirect slots outright, plain slots only
    /// for pointer params whose slot holds the address); regs resolving to
    /// a folded global base (literals plus dynamic terms move per edge, and
    /// SSA dominance keeps every term live on the incoming edge); a GEP
    /// over the phi's own dst (the loop-carried increment, resolved
    /// against this seed by the GEP fixpoint); a link-time-constant GEP
    /// over a global; or a select that folds right now. Anything else
    /// (notably a folded dynamic GEP reg) stays pending for the
    /// unresolvable-chain panic. Returns true when it seeded.
    fn seed_phi(
        f: &ir::Func,
        fname: &str,
        p: &ir::Phi,
        geps: &HashMap<String, ir::Gep>,
        selects: &HashMap<String, ir::Select>,
        resolved: &mut PtrResolution,
    ) -> bool {
        // Only a pointer-typed phi (`phi ptr [...]`) is a pointer VALUE; a
        // plain i16 value phi (clang emits `phi i8`/`phi i16` for value
        // merges everywhere) must never be seeded as an indirect slot.
        if !p.ptr {
            return false;
        }
        let key = ssa_key(fname, &p.dst);
        if resolved.contains_key(&key) {
            return false;
        }
        // A qualifying reg is one already seeded as a runtime-address slot
        // (`Base::Slot(_, true)` with any offset: its bytes live in a slot
        // the phi copy can move, folding k/terms onto them) or a plain
        // pointer PARAM (whose slot holds the address, `Base::Slot(_,
        // false)` per the ADR-009 ptr-param seeding).
        let param_holds_addr = |n: &str| f.params.iter().any(|p| p.name == *n && p.ptr);
        // A local object's own address (`alloca`): its slot IS the object,
        // so a pointer to it is the slot's own compile-time address, which
        // the backends materialize as literals like a global's.
        let is_alloca = |n: &str| {
            f.blocks.iter().any(|b| {
                b.insts
                    .iter()
                    .any(|i| matches!(i, ir::Inst::Alloca(a) if a.dst == n))
            })
        };
        let self_gep = |r: &str| {
            // A GEP over the phi's own dst is the loop-carried pointer
            // increment (`%18 = gep %7 +1` feeding `%7 = phi ptr [%18,
            // %5]`): its address bytes are the phi slot's bytes plus
            // k/terms, so it is a runtime address value once the phi seeds
            // as an indirect slot (the GEP fixpoint then resolves it
            // against that seed).
            matches!(
                geps.get(&ssa_key(fname, r)),
                Some(g) if g.base == ir::GepBase::Reg(p.dst.clone())
            )
        };
        let const_gep = |r: &str| {
            // A materialized GEP over a global with no dynamic terms is a
            // link-time constant address (an inlined `getelementptr` phi
            // arm): phi elimination moves its bytes as literals, so it
            // counts as a runtime address value like a bare global.
            matches!(
                geps.get(&ssa_key(fname, r)),
                Some(g)
                    if matches!(g.base, ir::GepBase::Global(_))
                        && g.terms.is_empty()
            )
        };
        // A select arm that folds right now (shared link-time base,
        // evaluated on demand because select folding otherwise runs after
        // phi seeding): its bytes move as literals plus dynamic terms,
        // like any folded global base.
        let foldable_select = |r: &str| -> bool {
            match selects.get(&ssa_key(fname, r)) {
                Some(s) => matches!(
                    fold_select(s, resolved, fname, geps),
                    Some((Base::Global(_), _, _))
                ),
                None => false,
            }
        };
        let runtime = p.incoming.iter().all(|(v, _)| match v {
            ir::Val::Const(_) | ir::Val::Global(_) => true,
            ir::Val::Reg(r) => match resolved.get(&ssa_key(fname, r)) {
                // An indirect slot holds address bytes; the edge copy
                // folds any k/terms onto them (or panics precisely where
                // the move shape is unsupported).
                Some((Base::Slot(_, true), _, _)) => true,
                // A pointer param's slot holds an address; an alloca's
                // slot IS the object, and its address is likewise a
                // compile-time constant (k/terms ride the edge copy).
                Some((Base::Slot(n, false), _, _)) if param_holds_addr(n) || is_alloca(n) => true,
                // A folded global base (a folded select or GEP over a
                // global): phi elimination moves its bytes as literals
                // plus dynamic terms, and SSA dominance keeps every term
                // live on the incoming edge by construction.
                Some((Base::Global(_), _, _)) => true,
                _ => self_gep(r) || const_gep(r) || foldable_select(r),
            },
        });
        if runtime && !p.incoming.is_empty() {
            resolved.insert(key, (Base::Slot(p.dst.clone(), true), 0, Vec::new()));
            return true;
        }
        false
    }

    // Pointer-typed phis seed as indirect slots when every
    // incoming is a runtime-address value (see `seed_phi`); the GEP
    // fixpoint below re-runs seeding to convergence for chains that
    // only resolve across passes (a phi feeding a GEP feeding a phi).
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut progressed = true;
        while progressed {
            progressed = false;
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Phi(p) = i {
                        if seed_phi(f, &fname, p, &geps, &selects, &mut resolved) {
                            progressed = true;
                        }
                    }
                }
            }
        }
    }
    // A call result used in pointer position (a GEP base, a load/store address
    // operand) is definitionally a pointer return: those LLVM operands are
    // always pointer-typed. Both backends copy the retval bytes into the call
    // dst slot on return, so the slot holds the two address bytes and the dst
    // dereferences as an indirect slot, the same model as the load-ptr seed
    // above (epic-cc#183). This is the shape `malloc` returns through
    // (epic-cc#343). A call result used only as a value is not seeded, so
    // integer math never miscompiles as addressing; anything else stays loud.
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut call_dsts = std::collections::HashSet::new();
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(c) = i {
                    if let Some(d) = &c.dst {
                        call_dsts.insert(d.clone());
                    }
                }
            }
        }
        if call_dsts.is_empty() {
            continue;
        }
        let mut seed = |r: &str| {
            if call_dsts.contains(r) {
                let key = ssa_key(&fname, r);
                if !resolved.contains_key(&key) {
                    resolved.insert(key, (Base::Slot(r.to_string(), true), 0, Vec::new()));
                }
            }
        };
        for (key, g) in &geps {
            if !key.starts_with(&format!("{fname}::")) {
                continue;
            }
            if let GepBase::Reg(r) = &g.base {
                seed(r);
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Load(l) => {
                        if let Some(r) = l.ptr.strip_prefix('%') {
                            seed(r);
                        }
                    }
                    Inst::Store(s) => {
                        if let Some(r) = s.ptr.strip_prefix('%') {
                            seed(r);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut pending: Vec<(String, ir::Gep)> = geps
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{fname}::")))
            .map(|(k, g)| (k.clone(), g.clone()))
            .collect();
        let mut pending_selects: Vec<(String, ir::Select)> = selects
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{fname}::")))
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect();
        let mut pending_phis: Vec<(String, ir::Phi)> = Vec::new();
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Phi(p) = i {
                    if p.ptr {
                        pending_phis.push((ssa_key(&fname, &p.dst), p.clone()));
                    }
                }
            }
        }
        let mut progressed = true;
        while progressed {
            progressed = false;
            let mut rest = Vec::new();
            for (key, g) in pending {
                match &g.base {
                    GepBase::Global(name) => {
                        assert!(
                            !resolved.contains_key(&key),
                            "iselcore: duplicate definition of pointer reg {key}"
                        );
                        resolved.insert(key, (Base::Global(name.clone()), g.k, g.terms.clone()));
                        progressed = true;
                    }
                    GepBase::Reg(r) => {
                        let rkey = ssa_key(&fname, r);
                        if let Some((b, kk, tt)) = resolved.get(&rkey).cloned() {
                            assert!(
                                !resolved.contains_key(&key),
                                "iselcore: duplicate definition of pointer reg {key}"
                            );
                            let mut terms = tt.clone();
                            terms.extend(g.terms.clone());
                            let k = g.k.checked_add(kk).unwrap_or_else(|| {
                                panic!("iselcore: gep offset overflow in {key}")
                            });
                            resolved.insert(key, (b, k, terms));
                            progressed = true;
                        } else if geps.contains_key(&rkey) || selects.contains_key(&rkey) {
                            rest.push((key, g));
                        } else if pending_phis.iter().any(|(k, _)| k == &rkey) {
                            // A GEP over a pointer phi that has not seeded
                            // yet (an LSR walk whose entry arm resolves in
                            // this same fixpoint, epic-cc#678) waits for the
                            // phi seeding below instead of dying: the seed
                            // unblocks it next round, and a phi that never
                            // seeds still fails loud at the stall check.
                            rest.push((key, g));
                        } else {
                            panic!("iselcore: no gep for pointer %{r} (chain base missing, key {rkey})");
                        }
                    }
                }
            }
            pending = rest;
            // Pointer-select pass: fold a select whose arms resolve to the
            // same base with matching term sets. The cond reg becomes a
            // scale-1 term, so `select c, base+kA, base+kB` (kA < kB) is
            // `base + kA + (kB-kA)×c`: c = 0 picks kA, c = 1 adds the
            // difference. A select whose arms are runtime address
            // VALUES that do not fold (distinct globals, a global vs a
            // runtime slot, two runtime slots) is itself a runtime address
            // VALUE: seed the dst as an indirect slot, whose bytes isel
            // materializes as a 2-byte value select. Only an arm that is
            // neither foldable nor a materializable runtime value (a
            // folded GEP reg, whose address is a link-time constant with no
            // slot bytes) stays pending and panics below.
            let mut rest_selects = Vec::new();
            for (key, s) in pending_selects {
                // A select whose arms are both runtime address CONSTANTS was
                // already seeded above as an indirect slot: the bytes the
                // select writes into the dst come from isel's value-select
                // materialization, not from a fold. Skip it here.
                if matches!((&s.a, &s.b), (ir::Val::Const(_), ir::Val::Const(_))) {
                    continue;
                }
                if let Some(folded) = fold_select(&s, &resolved, &fname, &geps) {
                    assert!(
                        !resolved.contains_key(&key),
                        "iselcore: duplicate definition of pointer reg {key}"
                    );
                    resolved.insert(key, folded);
                    progressed = true;
                } else if select_arm_is_runtime_value(&s.a, &resolved, &fname)
                    && select_arm_is_runtime_value(&s.b, &resolved, &fname)
                {
                    assert!(
                        !resolved.contains_key(&key),
                        "iselcore: duplicate definition of pointer reg {key}"
                    );
                    resolved.insert(key, (Base::Slot(s.dst.clone(), true), 0, Vec::new()));
                    progressed = true;
                } else {
                    rest_selects.push((key, s));
                }
            }
            pending_selects = rest_selects;
            // Phi seeding joins the fixpoint: a phi whose arms resolve
            // across passes (a phi feeding a GEP feeding a phi) seeds
            // here once its arms do, and its seed unblocks the GEPs
            // over it next round.
            let mut rest_phis = Vec::new();
            for (key, p) in pending_phis {
                if resolved.contains_key(&key) {
                    continue;
                }
                if seed_phi(f, &fname, &p, &geps, &selects, &mut resolved) {
                    progressed = true;
                    continue;
                }
                rest_phis.push((key, p));
            }
            pending_phis = rest_phis;
            // Pending phis stay out of the stall check on purpose: dead or
            // value-only phis (e.g. strchr's in the cc2 fixture) never seed
            // and isel never queries them, so stalling on them would fail
            // working programs. A phi that IS used but never seeds still
            // panics downstream at isel with its precise "no resolved base".
            if !progressed && (!pending.is_empty() || !pending_selects.is_empty()) {
                let gnames: Vec<&str> = pending.iter().map(|(k, _)| k.as_str()).collect();
                let snames: Vec<&str> = pending_selects.iter().map(|(k, _)| k.as_str()).collect();
                panic!(
                    "iselcore: cyclic or unresolvable pointer chain (geps {gnames:?}, selects {snames:?})"
                );
            }
        }
    }
    resolved
}

/// Fold a pointer-typed select whose two arms resolve to the same base with
/// identical term sets: `select i1 c, base+kA, base+kB` becomes
/// `(base, min(kA,kB), terms + (|kA-kB|, c))`. The cond's 0/1 polarity picks
/// the arm, so no inversion is needed. Returns `None` when the arms do not
/// fold to a common base, the term sets differ, or the cond is not a reg.
fn fold_select(
    s: &ir::Select,
    resolved: &PtrResolution,
    fname: &str,
    geps: &HashMap<String, ir::Gep>,
) -> Option<(Base, u16, Vec<(u16, String)>)> {
    // A materialized GEP over a global with no dynamic terms is a
    // link-time constant address (an inlined-`getelementptr` select
    // arm): fold it as its base plus offset instead of waiting for
    // the GEP fixpoint, which runs after select folding.
    let const_gep = |r: &str| -> Option<(Base, u16, Vec<(u16, String)>)> {
        match geps.get(&ssa_key(fname, r)) {
            Some(g) if matches!(g.base, ir::GepBase::Global(_)) && g.terms.is_empty() => {
                match &g.base {
                    ir::GepBase::Global(n) => Some((Base::Global(n.clone()), g.k, Vec::new())),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    let arm = |v: &ir::Val| -> Option<(Base, u16, Vec<(u16, String)>)> {
        match v {
            ir::Val::Reg(r) => resolved
                .get(&ssa_key(fname, r))
                .cloned()
                .or_else(|| const_gep(r)),
            ir::Val::Global(g) => Some((Base::Global(g.clone()), 0, Vec::new())),
            _ => None,
        }
    };
    let va = arm(&s.a)?;
    let vb = arm(&s.b)?;
    if va.0 != vb.0 || va.2 != vb.2 {
        return None;
    }
    let (lo, hi) = (va.1.min(vb.1), va.1.max(vb.1));
    let d = hi - lo;
    let c = match &s.cond {
        ir::Val::Reg(c) => c.clone(),
        _ => return None,
    };
    if d == 0 {
        // Both arms are the same pointer: the select is a no-op.
        return Some((va.0.clone(), lo, va.2.clone()));
    }
    let mut terms = va.2.clone();
    terms.push((d, c));
    Some((va.0.clone(), lo, terms))
}

/// Whether a pointer-select arm is a runtime address VALUE whose two bytes
/// isel can materialize into the dst slot: a `Const` literal, a `Global`
/// (its address is a link-time literal), or a reg resolving to a
/// runtime-address slot (`Base::Slot(_, true)`, whose bytes ARE the
/// address) or a plain global base (a link-time literal). A reg with a
/// constant offset or dynamic terms is a computed address with no single
/// materializable value and is not a runtime value.
fn select_arm_is_runtime_value(v: &ir::Val, resolved: &PtrResolution, fname: &str) -> bool {
    match v {
        ir::Val::Const(_) | ir::Val::Global(_) => true,
        ir::Val::Reg(r) => match resolved.get(&ssa_key(fname, r)) {
            Some((Base::Slot(_, true), 0, t)) | Some((Base::Global(_), 0, t)) if t.is_empty() => {
                true
            }
            _ => false,
        },
    }
}
