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

/// Fold every `Gep` and pointer-typed `Select` in `m` to `(base, k, terms)`.
/// `base` starts the chain, `k` adds the constant offset, `terms` appends
/// dynamic offsets inner-first, keyed `{func}::{reg}` via `ssa_key`.
/// Seeds (byval/sret params, allocas, runtime pointer values) anchor the
/// fixpoint; pointer selects fold when both arms share a base and term set,
/// else seed as indirect slots holding address bytes isel materializes.
/// A base that is neither a seed nor a pending entry breaks an earlier-stage
/// invariant and panics, as do unmaterializable arms and stalled scans.
pub fn resolve_pointers(m: &Module) -> HashMap<String, (Base, u8, Vec<(u8, String)>)> {
    let mut geps: HashMap<String, ir::Gep> = HashMap::new();
    let mut selects: HashMap<String, ir::Select> = HashMap::new();
    let mut resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
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
    // Pointer-typed phis whose every incoming is a runtime-address value
    // (a `Const` literal address or a register already seeded as a runtime
    // slot) are runtime addresses themselves: phi elimination copies the
    // incoming's two bytes into the dst slot per edge, so the dst can
    // dereference indirectly. A phi with any compile-time (folded) arm
    // stays pending for the unresolvable-chain panic below: its bytes do
    // not live in a slot.
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut progressed = true;
        while progressed {
            progressed = false;
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Phi(p) = i {
                        // Only a pointer-typed phi (`phi ptr [...]`) is a
                        // pointer VALUE; a plain i16 value phi (clang emits
                        // `phi i8`/`phi i16` for value merges everywhere)
                        // must never be seeded as an indirect slot.
                        if !p.ptr {
                            continue;
                        }
                        let key = ssa_key(&fname, &p.dst);
                        if resolved.contains_key(&key) {
                            continue;
                        }
                        // A qualifying reg is one already seeded as a
                        // runtime-address slot (`Base::Slot(_, true)` with
                        // no offset) or a plain pointer PARAM (whose slot
                        // holds the address, `Base::Slot(_, false)` per the
                        // ADR-009 ptr-param seeding): its bytes live in a
                        // slot the phi copy can move. A folded
                        // (compile-time) pointer reg has no slot and cannot
                        // be an incoming here.
                        let param_holds_addr =
                            |n: &str| f.params.iter().any(|p| p.name == *n && p.ptr);
                        let self_gep = |r: &str| {
                            // A GEP over the phi's own dst is the
                            // loop-carried pointer increment (`%18 = gep
                            // %7 +1` feeding `%7 = phi ptr [%18, %5]`):
                            // its address bytes are the phi slot's bytes
                            // plus k/terms, so it is a runtime address
                            // value once the phi seeds as an indirect slot
                            // (the GEP fixpoint then resolves it against
                            // that seed).
                            matches!(
                                geps.get(&ssa_key(&fname, r)),
                                Some(g) if g.base == ir::GepBase::Reg(p.dst.clone())
                            )
                        };
                        let runtime = p.incoming.iter().all(|(v, _)| match v {
                            ir::Val::Const(_) => true,
                            ir::Val::Reg(r) => match resolved.get(&ssa_key(&fname, r)) {
                                Some((Base::Slot(_, true), 0, t)) if t.is_empty() => true,
                                Some((Base::Slot(n, false), 0, t)) if t.is_empty() => {
                                    param_holds_addr(n)
                                }
                                _ => self_gep(r),
                            },
                            ir::Val::Global(_) => false,
                        });
                        if runtime && !p.incoming.is_empty() {
                            resolved.insert(key, (Base::Slot(p.dst.clone(), true), 0, Vec::new()));
                            progressed = true;
                        }
                    }
                }
            }
        }
    }
    // A call result used in pointer position (a GEP base, a load/store
    // address operand) is definitionally a pointer return: those LLVM
    // operands are always pointer-typed. Both backends copy the retval
    // bytes into the call dst slot on return, so the slot holds the two
    // address bytes and the dst dereferences as an indirect slot, the
    // same model as the load-ptr seed above (epic-cc#183). This is the
    // shape `malloc` returns through (epic-cc#343). A call result used
    // only as a value is not seeded, so integer math never miscompiles
    // as addressing; anything else stays a loud panic.
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
            // difference. The scale is the difference of two u8 offsets, so
            // it always fits. A select whose arms are runtime address
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
                if let Some(folded) = fold_select(&s, &resolved, &fname) {
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
    resolved: &HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    fname: &str,
) -> Option<(Base, u8, Vec<(u8, String)>)> {
    let arm = |v: &ir::Val| -> Option<(Base, u8, Vec<(u8, String)>)> {
        match v {
            ir::Val::Reg(r) => resolved.get(&ssa_key(fname, r)).cloned(),
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
fn select_arm_is_runtime_value(
    v: &ir::Val,
    resolved: &HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    fname: &str,
) -> bool {
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
