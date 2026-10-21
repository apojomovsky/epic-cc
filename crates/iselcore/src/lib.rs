//! `iselcore` — shared instruction-selection primitives used by both the
//! PIC14 (`isel`) and PIC18 (`isel-pic18`) backends.

pub mod chunk;
pub mod delay;
pub mod walk;

use ir::{Func, GepBase, Inst, Module};
use std::collections::{HashMap, HashSet};

/// The blocks each block of `f` dominates, keyed by the block's label.
/// The entry block dominates itself; every other block dominates the
/// blocks every predecessor path reaches.
pub fn func_dominators(f: &Func) -> HashMap<String, HashSet<String>> {
    let Some(entry) = f.blocks.first().map(|b| b.label.clone()) else {
        return HashMap::new();
    };
    let all: HashSet<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
    // Predecessor lists from the terminators' targets (the terminator is
    // the last inst of every block).
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &f.blocks {
        let mut targets: Vec<&str> = Vec::new();
        match b.insts.last() {
            Some(Inst::Br(br)) => targets.push(br.target.as_str()),
            Some(Inst::BrCond(bc)) => {
                targets.push(bc.t.as_str());
                targets.push(bc.f.as_str());
            }
            // A switch dispatches to every case and to the default.
            Some(Inst::Switch(sw)) => {
                for (_, l) in &sw.cases {
                    targets.push(l);
                }
                targets.push(&sw.default);
            }
            _ => {}
        }
        for t in targets {
            preds.entry(t).or_default().push(b.label.as_str());
        }
    }
    let mut dom: HashMap<String, HashSet<String>> = HashMap::new();
    dom.insert(entry.clone(), HashSet::from([entry.clone()]));
    for b in &f.blocks {
        if b.label != entry {
            dom.insert(b.label.clone(), all.clone());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            if b.label == entry {
                continue;
            }
            let mut new: HashSet<String> = match preds.get(b.label.as_str()) {
                Some(ps) => ps
                    .iter()
                    .map(|p| dom[*p].clone())
                    .reduce(|a, c| a.intersection(&c).cloned().collect())
                    .unwrap_or_else(|| all.clone()),
                None => all.clone(),
            };
            new.insert(b.label.clone());
            if new != dom[&b.label] {
                dom.insert(b.label.clone(), new);
                changed = true;
            }
        }
    }
    dom
}

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
pub fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// True when a comment-stripped, trimmed listing line is the bare directive
/// `name`: exactly `name`, or `name` followed by whitespace. A label such as
/// `endpoint:` or `listen: MOVLW 1` starts with a directive name but is not
/// one; a bare prefix test on those lines drops real code.
pub fn is_directive(line: &str, name: &str) -> bool {
    line == name
        || line
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with([' ', '\t']))
}

/// Where a local's bytes live. Only `Direct` is constructed: the `Frame`
/// case stays reserved (docs/internals/design/29 §2) so frame-pointer handling for
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

/// Parse alloc address-map text into addresses plus the staged-const set.
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become entries
/// (locals keyed `{func}::{name}`); `const <name>` lines name flash globals
/// with no RAM address and skip: isel reads their bytes from the `Module`.
/// `staged <name>` lines name consts isel stages through the shared buffer
/// (epic-cc#790). Both backends share this parser over alloc output:
/// nothing in it is PIC14-specific.
pub fn parse_map(text: &str) -> (HashMap<String, u16>, HashSet<String>) {
    let mut addrs = HashMap::new();
    let mut staged = HashSet::new();
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
            "staged" => {
                staged.insert(
                    it.next()
                        .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                        .to_string(),
                );
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
    (addrs, staged)
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
    /// unresolvable-chain panic. In the optimistic round (`optimistic` holds
    /// the still-unseeded phi keys) an arm that is itself an unresolved phi,
    /// gep, or select also counts: the fixpoint verifies it, and a chain
    /// that never resolves still stalls and panics. Returns true on seed.
    fn seed_phi(
        f: &ir::Func,
        fname: &str,
        p: &ir::Phi,
        geps: &HashMap<String, ir::Gep>,
        selects: &HashMap<String, ir::Select>,
        resolved: &mut PtrResolution,
        optimistic: Option<&HashSet<String>>,
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
        // Optimistic round only: an arm that is itself an unresolved phi,
        // gep, or select resolves once the seeds land (a swap partner, a
        // gep over one). Strict mode rejects these, so this runs solely on
        // programs the fixpoint would otherwise refuse.
        let optimistic_arm = |r: &str| {
            let Some(keys) = optimistic else {
                return false;
            };
            let rkey = ssa_key(fname, r);
            keys.contains(&rkey)
                || (geps.contains_key(&rkey) && !resolved.contains_key(&rkey))
                || (selects.contains_key(&rkey) && !resolved.contains_key(&rkey))
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
                _ => self_gep(r) || const_gep(r) || foldable_select(r) || optimistic_arm(r),
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
                        if seed_phi(f, &fname, p, &geps, &selects, &mut resolved, None) {
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
            // difference. A select whose arms are runtime address VALUES
            // that do not fold (distinct globals, a global vs a runtime
            // slot, two runtime slots, a nonzero-offset GEP arm) is itself
            // a runtime address VALUE: seed the dst as an indirect slot,
            // whose bytes isel materializes as a 2-byte value select. Only
            // an arm that is neither foldable nor a materializable runtime
            // value (a dynamic GEP reg or an offset over a runtime slot)
            // stays pending and panics below.
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
                if seed_phi(f, &fname, &p, &geps, &selects, &mut resolved, None) {
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
            // Optimistic round (epic-cc#1127): a phi cycle whose arms are
            // each other's pending phis or geps (a pointer swap, one arm a
            // gep over its partner) stalls strict seeding forever. Seed
            // those phis at once; the next rounds verify every chain, and
            // anything still stuck panics below as before.
            if !progressed && (!pending.is_empty() || !pending_selects.is_empty()) {
                let keys: HashSet<String> = pending_phis.iter().map(|(k, _)| k.clone()).collect();
                let mut rest = Vec::new();
                for (key, p) in pending_phis {
                    if resolved.contains_key(&key) {
                        continue;
                    }
                    if seed_phi(f, &fname, &p, &geps, &selects, &mut resolved, Some(&keys)) {
                        progressed = true;
                    } else {
                        rest.push((key, p));
                    }
                }
                pending_phis = rest;
            }
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
/// runtime-address slot (`Base::Slot(_, true)` with no offset, whose bytes
/// ARE the address) or a global base (a link-time literal plus the constant
/// offset, epic-cc#781). A reg with dynamic terms, or an offset over a
/// slot, is a computed address with no single materializable value.
fn select_arm_is_runtime_value(v: &ir::Val, resolved: &PtrResolution, fname: &str) -> bool {
    match v {
        ir::Val::Const(_) | ir::Val::Global(_) => true,
        ir::Val::Reg(r) => match resolved.get(&ssa_key(fname, r)) {
            Some((Base::Slot(_, true), 0, t)) if t.is_empty() => true,
            Some((Base::Global(_), _, t)) if t.is_empty() => true,
            _ => false,
        },
    }
}

/// How one `Load` keeps its byte out of an overlay slot (epic-cc#863).
/// `Direct` feeds a single ALU op, which reads the source global as its
/// memory operand. `ThreadW` feeds two or more stores, which write back
/// the byte the load left in W, naming source then destinations. Both
/// preserve the single volatile read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadFold {
    Direct(String),
    ThreadW(String, Vec<String>),
}

/// Producer regs needing no overlay slot. `loads` maps a folded load to
/// its source (and stores). `forwarded` maps a single-use `Load`/`Bin`
/// result computed straight into a store destination to that store's
/// global. `alloc` drops both sets; `isel-pic18` skips their staged
/// copies. One predicate serves both so a dropped slot is never read.
#[derive(Clone, Debug, Default)]
pub struct ValueFolds {
    pub loads: HashMap<String, LoadFold>,
    pub forwarded: HashMap<String, String>,
}

impl ValueFolds {
    /// Every reg with no slot: folded loads plus forwarded producers.
    pub fn unplaced(&self) -> HashSet<String> {
        let mut out: HashSet<String> = self.forwarded.keys().cloned().collect();
        out.extend(self.loads.keys().cloned());
        out
    }
    /// Regs needing no slot on classic PIC14 (epic-cc#875) and PIC14E
    /// (epic-cc#902): folded loads, forwarded regs produced by a `Load`
    /// whose store reads the source global, and single-byte forwarded
    /// `Bin` results, which both backends compute straight into the
    /// store destination (epic-cc#862).
    pub fn unplaced_pic14(&self, f: &ir::Func) -> HashSet<String> {
        let mut out: HashSet<String> = self.loads.keys().cloned().collect();
        for r in self.forwarded.keys() {
            let produced_by_load = f
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::Load(l) if l.dst == *r));
            if produced_by_load {
                out.insert(r.clone());
            }
        }
        out.extend(pic14_slotless_bins(f, self).keys().cloned());
        out
    }
}

/// Regs a single-byte `Bin` computes straight into its store destination on
/// PIC14/PIC14E (epic-cc#862): the `forwarded` producer is a one-byte
/// Add/Sub/And/Xor whose only use is the direct-global store, and every
/// one-byte lowering writes its destination once with no readback, so the
/// backend addresses the store global instead of a temp slot. Wider bins
/// stay placed: their lowerings accumulate in place, and those
/// intermediate values must never touch a live global. So do all-const
/// operands: the backend has no lowering for those (it panics), so the
/// fold provably never applies to them.
pub fn pic14_slotless_bins(f: &ir::Func, folds: &ValueFolds) -> HashMap<String, String> {
    folds
        .forwarded
        .iter()
        .filter(|(r, _)| {
            f.blocks.iter().flat_map(|b| &b.insts).any(|i| {
                matches!(i, Inst::Bin(q) if &q.dst == *r && q.ty.bytes() == 1
                    && !matches!((&q.a, &q.b), (ir::Val::Const(_), ir::Val::Const(_))))
            })
        })
        .map(|(r, g)| (r.clone(), g.clone()))
        .collect()
}

/// A gap instruction the fold may move a global read past: a pure lane
/// copy with no memory behavior. Anything reading or writing memory, or
/// calling out, disqualifies the fold.
fn fold_gap_pure(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Zext(_)
            | Inst::Sext(_)
            | Inst::Trunc(_)
            | Inst::Freeze(_)
            | Inst::Select(_)
            | Inst::Gep(_)
            | Inst::IntToPtr(_)
            | Inst::Alloca(_)
            | Inst::Phi(_)
    )
}

/// The `Store`/`Load`/call span test `isel-pic18` folds under
/// (epic-cc#723): no memory behavior may sit between producer and
/// consumer, since the folded access moves to the producer.
fn fold_span_clean(insts: &[Inst]) -> bool {
    insts.iter().all(|i| match i {
        Inst::Call(_)
        | Inst::Asm(_)
        | Inst::Store(_)
        | Inst::Memcpy(_)
        | Inst::VaStart(_)
        | Inst::VaArg(_)
        | Inst::Load(_)
        | Inst::Br(_)
        | Inst::BrCond(_)
        | Inst::Switch(_)
        | Inst::Ret(_, _) => false,
        _ => true,
    })
}

/// Load/store folds for one function, shared by `alloc` and
/// `isel-pic18` (epic-cc#863). Single-use loads feeding ALU operands
/// read the source global (`Direct`, never into `Or`), loads feeding
/// only stores ride W (`ThreadW`), and single-use producers feeding
/// stores compute into the destination (`forwarded`). A gapped load
/// co-folds past a sister load into the same op when the b-side
/// load leads, keeping volatile order (epic-cc#876).
pub fn find_value_folds(
    f: &ir::Func,
    m: &Module,
    resolved: &PtrResolution,
    access_safe: Option<&HashSet<String>>,
) -> ValueFolds {
    let mut out = ValueFolds::default();
    let Some(safe) = access_safe else {
        return out;
    };
    let mut uses: HashMap<String, usize> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            for r in ir::read_vals(inst) {
                if r.is_empty() {
                    continue;
                }
                *uses.entry(r).or_insert(0) += 1;
            }
        }
    }
    // A load source both stages read as RAM: a known mutable global.
    // Flash consts read through TBLRD, so they never fold. Bit entities
    // (epic-cc#707) are not RAM either: folding would drop the bit-op
    // lowering and read a byte that is never placed.
    let ram_source = |ptr: &str| -> Option<String> {
        let g = ptr.strip_prefix('@')?;
        let known = m.globals.iter().find(|x| x.name == g)?;
        if known.is_const || known.addr.is_some_and(|p| p.bit.is_some()) {
            return None;
        }
        Some(g.to_string())
    };
    // Def site of a load/binop producer: (block, inst), function-wide.
    let mut def_at: HashMap<&str, (usize, usize)> = HashMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, inst) in b.insts.iter().enumerate() {
            match inst {
                Inst::Load(l) => {
                    def_at.insert(l.dst.as_str(), (bi, ii));
                }
                Inst::Bin(q) => {
                    def_at.insert(q.dst.as_str(), (bi, ii));
                }
                _ => {}
            }
        }
    }
    // Shape 1: single-use load into one ALU operand. Two passes: the
    // first folds over pure gaps; the second co-folds a declined load
    // past the sister load that folded into the same op (epic-cc#876).
    // The sister always folds first (its gap sits past the candidate
    // on an order-compatible pair), so two passes are complete.
    for cofold in [false, true] {
        let mut pending: Vec<(String, LoadFold)> = Vec::new();
        for b in &f.blocks {
            for (ci, inst) in b.insts.iter().enumerate() {
                let Inst::Bin(q) = inst else { continue };
                // `Or` stays staged: its binops feed the boolean lanes, whose
                // accumulator reads the slot. Folding it would starve the lane.
                if !matches!(
                    q.op,
                    ir::BinOp::Add | ir::BinOp::Sub | ir::BinOp::And | ir::BinOp::Xor
                ) {
                    continue;
                }
                let n = q.ty.bytes();
                for operand in [&q.a, &q.b] {
                    let ir::Val::Reg(r) = operand else { continue };
                    if uses.get(r).copied().unwrap_or(0) != 1 {
                        continue;
                    }
                    let Some(&(bi, pi)) = def_at.get(r.as_str()) else {
                        continue;
                    };
                    if f.blocks[bi].label != b.label || pi >= ci {
                        continue;
                    }
                    let Inst::Load(l) = &f.blocks[bi].insts[pi] else {
                        continue;
                    };
                    let Some(g) = ram_source(&l.ptr) else {
                        continue;
                    };
                    // A banked source would trade its bankless `MOVFF` for a
                    // `MOVLB` word. Only access-bank sources fold.
                    if !safe.contains(&g) {
                        continue;
                    }
                    if l.ptr_ty || l.ty.bytes() != n {
                        continue;
                    }
                    if resolved.contains_key(&ssa_key(&f.name, r)) {
                        continue;
                    }
                    // A lane may claim an icmp-adjacent load as its direct
                    // field, reading the source a second time. Stay out.
                    if matches!(b.insts.get(pi + 1), Some(Inst::Icmp(_))) {
                        continue;
                    }
                    // A gap load is transparent on the co-fold pass when it
                    // is the sister operand's fold: both backends read the b
                    // side first, so the pair keeps program order exactly
                    // when the b-side load leads. Past one byte, two volatile
                    // reads would interleave, so that pair stays staged.
                    let gap_ok =
                        f.blocks[bi].insts[pi + 1..ci]
                            .iter()
                            .enumerate()
                            .all(|(k, inst)| {
                                if fold_gap_pure(inst) {
                                    return true;
                                }
                                if !cofold {
                                    return false;
                                }
                                let Inst::Load(sl) = inst else { return false };
                                if !matches!(out.loads.get(&sl.dst), Some(LoadFold::Direct(_))) {
                                    return false;
                                }
                                let cand_a = matches!(&q.a, ir::Val::Reg(x) if x == r);
                                let sister_other = if cand_a {
                                    matches!(&q.b, ir::Val::Reg(x) if x == &sl.dst)
                                } else {
                                    matches!(&q.a, ir::Val::Reg(x) if x == &sl.dst)
                                };
                                if !sister_other {
                                    return false;
                                }
                                let spi = pi + 1 + k;
                                if (cand_a && spi >= pi) || (!cand_a && pi >= spi) {
                                    return false;
                                }
                                if n > 1 && l.volatile && sl.volatile {
                                    return false;
                                }
                                true
                            });
                    if !gap_ok {
                        continue;
                    }
                    pending.push((r.clone(), LoadFold::Direct(g)));
                }
            }
        }
        out.loads.extend(pending);
    }
    // Shape 2: one load fanned out to stores only. W carries the byte:
    // the gap holds nothing but the consuming stores, each of which
    // preserves W.
    for b in f.blocks.iter() {
        for (pi, inst) in b.insts.iter().enumerate() {
            let Inst::Load(l) = inst else { continue };
            let Some(g) = ram_source(&l.ptr) else {
                continue;
            };
            if l.ptr_ty || l.ty.bytes() != 1 {
                continue;
            }
            if resolved.contains_key(&ssa_key(&f.name, &l.dst)) {
                continue;
            }
            if uses.get(&l.dst).copied().unwrap_or(0) < 2 {
                continue;
            }
            // Both the read and every write go direct, so the source
            // must be bank-select free; destinations check below.
            if !safe.contains(&g) {
                continue;
            }
            let mut last = pi;
            let mut ok = true;
            let mut dsts: Vec<String> = Vec::new();
            for (si, other) in b.insts.iter().enumerate() {
                if si == pi {
                    continue;
                }
                let used_here = ir::read_vals(other).contains(&l.dst);
                if si < pi && used_here {
                    ok = false;
                    break;
                }
                if si > pi && used_here {
                    let Inst::Store(s) = other else {
                        ok = false;
                        break;
                    };
                    if !matches!(&s.val, ir::Val::Reg(v) if v == &l.dst) {
                        ok = false;
                        break;
                    }
                    if !s.ptr.starts_with('@') || s.ty.bytes() != 1 {
                        ok = false;
                        break;
                    }
                    // A banked destination would trade its bankless
                    // `MOVFF` for a `MOVLB` word. Only access-bank
                    // destinations thread.
                    let dst = s.ptr.strip_prefix('@').expect("checked @-form above");
                    if !safe.contains(dst) {
                        ok = false;
                        break;
                    }
                    dsts.push(dst.to_string());
                    last = last.max(si);
                }
            }
            if !ok {
                continue;
            }
            // Uses in other blocks, or any gap instruction that is not
            // one of the consuming stores, keep the staged copy.
            let elsewhere = uses.get(&l.dst).copied().unwrap_or(0)
                != b.insts
                    .iter()
                    .filter(|i| ir::read_vals(i).contains(&l.dst))
                    .count();
            if elsewhere {
                continue;
            }
            if !b.insts[pi + 1..last].iter().all(|i| match i {
                Inst::Store(s) => matches!(&s.val, ir::Val::Reg(v) if v == &l.dst),
                _ => false,
            }) {
                continue;
            }
            out.loads.insert(l.dst.clone(), LoadFold::ThreadW(g, dsts));
        }
    }
    // Shape 3: single-use load/binop into a direct-global store. The
    // epic-cc#723 subset `alloc` can verify without addresses: wide
    // staged copies and non-global destinations stay placed.
    for b in &f.blocks {
        for (si, inst) in b.insts.iter().enumerate() {
            let Inst::Store(s) = inst else { continue };
            let ir::Val::Reg(r) = &s.val else { continue };
            if uses.get(r).copied().unwrap_or(0) != 1 {
                continue;
            }
            let Some(dst) = s.ptr.strip_prefix('@') else {
                continue;
            };
            let Some(known) = m.globals.iter().find(|x| x.name == dst) else {
                continue;
            };
            // A bit entity (epic-cc#707) takes no forwarded computation:
            // the producer stages and the store serves a bit op.
            if known.is_const || known.addr.is_some_and(|p| p.bit.is_some()) {
                continue;
            }
            let Some(&(pbi, pi)) = def_at.get(r.as_str()) else {
                continue;
            };
            if f.blocks[pbi].label != b.label || pi >= si {
                continue;
            }
            // A lane target or a lane-eaten intermediate keeps its
            // slot: `Or` binops feed the boolean lanes, and a load
            // feeding an adjacent compare may become a lane field.
            match &f.blocks[pbi].insts[pi] {
                Inst::Load(l) => {
                    if l.ty.bytes() != s.ty.bytes() || l.ptr_ty {
                        continue;
                    }
                    if ram_source(&l.ptr).is_none() {
                        continue;
                    }
                    if l.ty.bytes() != 1 {
                        continue;
                    }
                    if matches!(b.insts.get(pi + 1), Some(Inst::Icmp(_))) {
                        continue;
                    }
                }
                Inst::Bin(q) => {
                    if q.ty.bytes() != s.ty.bytes() {
                        continue;
                    }
                    if !matches!(
                        q.op,
                        ir::BinOp::Add | ir::BinOp::Sub | ir::BinOp::And | ir::BinOp::Xor
                    ) {
                        continue;
                    }
                }
                _ => continue,
            }
            if !fold_span_clean(&f.blocks[pbi].insts[pi + 1..si]) {
                continue;
            }
            out.forwarded.insert(r.clone(), dst.to_string());
        }
    }
    out
}

/// Whether `emit_load_w` of pointer reg `v` writes STATUS,C for any lane.
///
/// The one `emit_load_w` arm that writes `C` is the resolved-GEP address
/// materialization of an indirect or pointer-param slot with a constant
/// offset or dynamic terms. The shared-exit borrow chain holds `C` across
/// lanes, so a compare whose rhs load writes it keeps the per-lane cascade.
pub fn load_w_writes_carry(
    fname: &str,
    params: &[ir::Param],
    resolved: &PtrResolution,
    v: &ir::Val,
) -> bool {
    let ir::Val::Reg(r) = v else {
        return false;
    };
    let Some((base, k, terms)) = resolved.get(&ssa_key(fname, r)) else {
        return false;
    };
    let Base::Slot(sname, indirect) = base else {
        return false;
    };
    let holds_addr = *indirect || params.iter().any(|pp| pp.name == *sname && pp.ptr);
    holds_addr && (*k != 0 || !terms.is_empty())
}

/// The `icmp` a block's `BrCond` absorbs on PIC18 (epic-cc#862): the block
/// tail is `[.., Icmp, BrCond]` on the compare's result, the compare is
/// the result's only use in the function, its width and predicate have a
/// fused lowering, and the unsigned carry-chain guard holds. A fused
/// compare branches straight to the exits and never writes its 0/1 slot.
///
/// `signed_ok` decides the signed predicates, whose chain guard reads slot
/// addresses: `isel-pic18` answers from the final map, `alloc` answers
/// `false` so it keeps the slot whenever the answer depends on layout.
pub fn fusable_icmp<'f>(
    f: &'f ir::Func,
    b: &'f ir::Block,
    resolved: &PtrResolution,
    signed_ok: impl Fn(&ir::Icmp) -> bool,
) -> Option<&'f ir::Icmp> {
    let [.., Inst::Icmp(c), Inst::BrCond(bc)] = b.insts.as_slice() else {
        return None;
    };
    let ir::Val::Reg(cond) = &bc.cond else {
        return None;
    };
    if c.dst != *cond {
        return None;
    }
    if !matches!(c.ty.bytes(), 1 | 2 | 4) {
        return None;
    }
    if !matches!(
        c.pred.as_str(),
        "eq" | "ne" | "ult" | "uge" | "ugt" | "ule" | "slt" | "sle" | "sgt" | "sge"
    ) {
        return None;
    }
    if matches!(c.pred.as_str(), "ult" | "uge" | "ugt" | "ule")
        && load_w_writes_carry(&f.name, &f.params, resolved, &c.b)
    {
        return None;
    }
    if matches!(c.pred.as_str(), "slt" | "sle" | "sgt" | "sge") && !signed_ok(c) {
        return None;
    }
    let bi_own = f.blocks.iter().position(|bb| std::ptr::eq(bb, b))?;
    let branch_idx = b.insts.len() - 1;
    for (bi, bb) in f.blocks.iter().enumerate() {
        for (ii, inst) in bb.insts.iter().enumerate() {
            if bi == bi_own && ii == branch_idx {
                continue;
            }
            if ir::read_vals(inst).iter().any(|v| v == cond) {
                return None;
            }
        }
    }
    Some(c)
}

/// The operand loads a fused compare reads in place (epic-cc#862): per
/// side `0` (`a`) or `1` (`b`), a single-use same-block `Load` of the
/// compare's width from a global `global_ok` accepts becomes that global,
/// and the load emits nothing. A gap holding a call, an asm blob, or any
/// memory write keeps the load staged: it may be volatile, and its
/// position must not move past observable behavior. Returns
/// `(side, load dst, global)` in side order.
pub fn fused_chain_loads(
    f: &ir::Func,
    b: &ir::Block,
    c: &ir::Icmp,
    global_ok: impl Fn(&str, &ir::Icmp) -> bool,
) -> Vec<(u8, String, String)> {
    let mut out = Vec::new();
    let ci = b
        .insts
        .iter()
        .position(|i| matches!(i, Inst::Icmp(x) if x.dst == c.dst))
        .unwrap_or(usize::MAX);
    for (side, v) in [(0u8, &c.a), (1u8, &c.b)] {
        let ir::Val::Reg(r) = v else { continue };
        let Some(li) = b
            .insts
            .iter()
            .position(|i| matches!(i, Inst::Load(l) if l.dst == *r))
        else {
            continue;
        };
        if li >= ci {
            continue;
        }
        let Inst::Load(l) = &b.insts[li] else {
            continue;
        };
        if l.ty.bytes() != c.ty.bytes() || l.ptr_ty {
            continue;
        }
        let Some(name) = l.ptr.strip_prefix('@') else {
            continue;
        };
        if !global_ok(name, c) {
            continue;
        }
        let users = f
            .blocks
            .iter()
            .flat_map(|bb| bb.insts.iter())
            .filter(|inst| !matches!(inst, Inst::Icmp(x) if x.dst == c.dst))
            .filter(|inst| ir::read_vals(inst).iter().any(|u| u == r))
            .count();
        if users != 0 {
            continue;
        }
        let gap_clean = b.insts[li + 1..ci].iter().all(|inst| {
            !matches!(
                inst,
                Inst::Call(_)
                    | Inst::Asm(_)
                    | Inst::Store(_)
                    | Inst::Memcpy(_)
                    | Inst::VaStart(_)
                    | Inst::Br(_)
                    | Inst::BrCond(_)
                    | Inst::Switch(_)
            )
        });
        if !gap_clean {
            continue;
        }
        out.push((side, r.clone(), name.to_string()));
    }
    out
}

/// Regs `isel-pic18` never gives a slot because a fused compare absorbs
/// them (epic-cc#862): each absorbed `icmp` result and the operand loads
/// it reads in place. Every decision that depends on the final layout
/// (signed chains, SFR-resident sources, flash or packed globals) answers
/// "keep the slot" here, so this set is a subset of what the backend
/// fuses and a dropped slot is never read.
pub fn fused_compare_regs(
    f: &ir::Func,
    m: &Module,
    resolved: &PtrResolution,
    packed: &HashMap<String, u8>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for b in &f.blocks {
        let Some(c) = fusable_icmp(f, b, resolved, |_| false) else {
            continue;
        };
        out.insert(c.dst.clone());
        let ram_global = |name: &str, c: &ir::Icmp| {
            !matches!(c.pred.as_str(), "slt" | "sle" | "sgt" | "sge")
                && !packed.contains_key(name)
                && m.globals.iter().any(|g| g.name == name && !g.is_const)
        };
        out.extend(
            fused_chain_loads(f, b, c, ram_global)
                .into_iter()
                .map(|(_, reg, _)| reg),
        );
    }
    out
}
/// Const producers `isel-pic18` threads through W instead of a slot
/// (epic-cc#862): a single-phi block whose one-byte phi takes only
/// constant incomings from every CFG predecessor and whose only use is
/// the direct-global store right after the phis, or a one-byte
/// two-constant `select` consumed by the next instruction, a
/// direct-global store. Each edge/arm emits just `MOVLW` and the store
/// one `MOVWF`, so no slot is addressed. The single-use pin is what
/// makes the subset exact: every competing fold (boolean lanes, trunc
/// and call-arg forwards) observes the value through a syntactic read
/// or through the dst slot itself, so with the store as the only read
/// no other path can claim it.
pub fn const_w_phi_regs(
    f: &ir::Func,
    m: &Module,
    resolved: &PtrResolution,
    packed: &HashMap<String, u8>,
) -> HashSet<String> {
    let mut uses: HashMap<String, usize> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            for r in ir::read_vals(inst) {
                if r.is_empty() {
                    continue;
                }
                *uses.entry(r).or_insert(0) += 1;
            }
        }
    }
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &f.blocks {
        let mut targets: Vec<&str> = Vec::new();
        match b.insts.last() {
            Some(Inst::Br(br)) => targets.push(&br.target),
            Some(Inst::BrCond(bc)) => {
                targets.push(&bc.t);
                targets.push(&bc.f);
            }
            Some(Inst::Switch(sw)) => {
                for (_, l) in &sw.cases {
                    targets.push(l);
                }
                targets.push(&sw.default);
            }
            _ => {}
        }
        for t in targets {
            preds.entry(t).or_default().push(b.label.as_str());
        }
    }
    let mut out = HashSet::new();
    let store_ok = |s: &ir::Store| {
        if s.ty.bytes() != 1 {
            return None;
        }
        let dst = s.ptr.strip_prefix('@')?;
        let known = m.globals.iter().find(|x| x.name == dst)?;
        if known.is_const || packed.contains_key(dst) {
            return None;
        }
        Some(())
    };
    for b in &f.blocks {
        let mut phis = b.insts.iter().filter_map(|i| match i {
            Inst::Phi(p) => Some(p),
            _ => None,
        });
        let Some(p) = phis.next() else { continue };
        if phis.next().is_some() || p.ptr || p.ty.bytes() != 1 {
            continue;
        }
        if p.incoming.is_empty()
            || p.incoming
                .iter()
                .any(|(v, _)| !matches!(v, ir::Val::Const(_)))
        {
            continue;
        }
        if uses.get(&p.dst).copied().unwrap_or(0) != 1 {
            continue;
        }
        if resolved.contains_key(&ssa_key(&f.name, &p.dst)) {
            continue;
        }
        let mut froms: Vec<&str> = p.incoming.iter().map(|(_, fr)| fr.as_str()).collect();
        froms.sort_unstable();
        let mut ps: Vec<&str> = preds.get(b.label.as_str()).cloned().unwrap_or_default();
        ps.sort_unstable();
        ps.dedup();
        if froms != ps {
            continue;
        }
        let mut rest = b.insts.iter().filter(|i| !matches!(i, Inst::Phi(_)));
        let Some(Inst::Store(s)) = rest.next() else {
            continue;
        };
        if !matches!(&s.val, ir::Val::Reg(r) if r == &p.dst) {
            continue;
        }
        if store_ok(s).is_none() {
            continue;
        }
        out.insert(p.dst.clone());
    }
    for b in &f.blocks {
        for (ii, inst) in b.insts.iter().enumerate() {
            let Inst::Select(s) = inst else { continue };
            if s.ptr || s.ty.bytes() != 1 || matches!(s.cond, ir::Val::Const(_)) {
                continue;
            }
            if !matches!((&s.a, &s.b), (ir::Val::Const(_), ir::Val::Const(_))) {
                continue;
            }
            if uses.get(&s.dst).copied().unwrap_or(0) != 1 {
                continue;
            }
            if resolved.contains_key(&ssa_key(&f.name, &s.dst)) {
                continue;
            }
            let Some(Inst::Store(st)) = b.insts.get(ii + 1) else {
                continue;
            };
            if !matches!(&st.val, ir::Val::Reg(r) if r == &s.dst) {
                continue;
            }
            if store_ok(st).is_none() {
                continue;
            }
            out.insert(s.dst.clone());
        }
    }
    out
}

/// Drop `forwarded` entries whose producer is a load from a packed flag
/// global (epic-cc#1162). The packed load still needs its slot: the
/// backend materializes the bit there for its consumers, so a forwarded
/// (slotless) producer would leave the store reading nothing. The fold
/// shapes that read whole bytes exclude packed sources through the fold
/// gate; this covers the forward shape, which never consults it. Empty
/// `packed` is a no-op.
pub fn drop_packed_forwards(
    folds: &mut ValueFolds,
    f: &ir::Func,
    packed: &std::collections::HashMap<String, u8>,
) {
    if packed.is_empty() {
        return;
    }
    folds.forwarded.retain(|r, _| {
        !f.blocks.iter().flat_map(|b| &b.insts).any(|i| {
            matches!(i, ir::Inst::Load(l) if &l.dst == r
                && l.ptr.strip_prefix('@').is_some_and(|g| packed.contains_key(g)))
        })
    });
}

/// Whether a const global joins the pooled flash string table
/// (epic-cc#815): unmapped, non-empty, and fitting one RETLW chunk.
/// `build_pool` and alloc's copy gating both call this, so membership
/// agrees by construction instead of by duplicated filters.
pub fn pool_member(g: &ir::Global) -> bool {
    g.is_const && g.addr.is_none() && !g.bytes.is_empty() && g.bytes.len() <= 255
}

/// Callee whose byte loop isel specializes to pool reads (epic-cc#817).
/// Direct calls with a pool-proven fmt arg route to the variant below;
/// every other call keeps the original RAM loop.
pub const LOG_POOL_CALLEE: &str = "epic_harness_log";
/// Synthesized pool variant of the log loop, emitted inside the callee's
/// own extent over its frame. Slots hold (in-chunk index, chunk id)
/// pairs there, so only routed (pool-proven) call sites may target it.
pub const LOG_POOL_VARIANT: &str = "__epic_log_pool";

/// Whether a call routes to the pool log variant (epic-cc#817): a direct
/// call to the log callee whose fmt arg is pool-proven. `pooled` tests
/// pool membership; alloc passes its address-taken snapshot, isel its
/// pool keys, so gating and routing agree by construction.
pub fn log_pool_site(
    m: &Module,
    resolved: &PtrResolution,
    pooled: &dyn Fn(&str) -> bool,
    caller: &str,
    func: &str,
    args: &[ir::CallArg],
    callees: &[String],
) -> bool {
    if func != LOG_POOL_CALLEE || !callees.is_empty() {
        return false;
    }
    let Some(arg) = args.first() else {
        return false;
    };
    if arg.ty.is_some() || arg.byval.is_some() || arg.sret {
        return false;
    }
    pool_proven_ptr(m, resolved, pooled, caller, &arg.val, 0)
}

/// Whether a pointer-select's arms all read through the pool
/// (epic-cc#817): both arms pool-proven and every use of the dst is a
/// routed log fmt arg, so the arms keep no RAM copy. Any other use
/// keeps the status-quo copy; a dead select (no uses) counts as fully
/// routed, since nothing can read it.
pub fn select_pool_routed(
    m: &Module,
    resolved: &PtrResolution,
    pooled: &dyn Fn(&str) -> bool,
    func: &str,
    dst: &str,
) -> bool {
    let Some(s) = select_of(m, func, dst) else {
        return false;
    };
    if !s.ptr {
        return false;
    }
    if !pool_proven_ptr(m, resolved, pooled, func, &s.a, 0)
        || !pool_proven_ptr(m, resolved, pooled, func, &s.b, 0)
    {
        return false;
    }
    // Regs are function-scoped, so only the owning function can use the
    // dst; scanning the whole module would match same-named regs in
    // other functions and pin every select to RAM.
    let Some(f) = m.funcs.iter().find(|f| f.name == func) else {
        return false;
    };
    for b in &f.blocks {
        for inst in &b.insts {
            if !reads_reg(inst, dst) {
                continue;
            }
            let routed = match inst {
                Inst::Call(c) => {
                    matches!(c.args.first(), Some(a) if a.val == ir::Val::Reg(dst.to_string()))
                        && log_pool_site(m, resolved, pooled, &f.name, &c.func, &c.args, &c.callees)
                }
                _ => false,
            };
            if !routed {
                return false;
            }
        }
    }
    true
}

/// Whether a value is a pool-resident address (epic-cc#817): a pooled
/// member directly, a clean use of one (constant offsets stay inside
/// the member's chunk, so the variant's index arithmetic serves them),
/// or a select over two such arms (dynamic terms over a select still
/// route: the prologue resolves the runtime address). Only a direct
/// pooled global with dynamic terms stays on the RAM loop.
fn pool_proven_ptr(
    m: &Module,
    resolved: &PtrResolution,
    pooled: &dyn Fn(&str) -> bool,
    func: &str,
    val: &ir::Val,
    depth: u8,
) -> bool {
    if depth > 4 {
        return false;
    }
    match val {
        ir::Val::Global(g) => pooled(g),
        ir::Val::Reg(r) => match resolved.get(&ssa_key(func, r)) {
            Some((Base::Global(g), _, t)) => t.is_empty() && pooled(g),
            Some((Base::Slot(dst, _), _, _)) => select_of(m, func, dst).is_some_and(|s| {
                s.ptr
                    && pool_proven_ptr(m, resolved, pooled, func, &s.a, depth + 1)
                    && pool_proven_ptr(m, resolved, pooled, func, &s.b, depth + 1)
            }),
            _ => false,
        },
        _ => false,
    }
}

/// The pointer-select defining `dst` in `func`, if any.
fn select_of<'a>(m: &'a Module, func: &str, dst: &str) -> Option<&'a ir::Select> {
    m.funcs
        .iter()
        .find(|f| f.name == func)?
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Select(s) if s.dst == dst => Some(s),
            _ => None,
        })
}

/// Whether `inst` reads `reg` in any operand position, phi incomings
/// included. Defs never count: only uses can pin a select to RAM.
fn reads_reg(inst: &Inst, reg: &str) -> bool {
    ir::read_vals(inst)
        .iter()
        .any(|v| v.strip_prefix('%').unwrap_or(v) == reg)
}

/// Whether a pointer passed as arg `idx` to direct call `callee` may be
/// dereferenced by the callee (epic-cc#1026). Declared-only callees, byval
/// params, and any read of the param in the body count as reads.
pub fn callee_reads_param(m: &Module, callee: &str, idx: usize) -> bool {
    let Some(f) = m.funcs.iter().find(|f| f.name == callee) else {
        return true;
    };
    let Some(p) = f.params.get(idx) else {
        return true;
    };
    if p.byval.is_some() {
        return true;
    }
    let name = p.name.strip_prefix('%').unwrap_or(&p.name);
    // A body-less definition is a declaration, so its reads are unknown.
    if f.blocks.is_empty() {
        return true;
    }
    f.blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| reads_reg(i, name)))
}

/// Flash-pointer provenance for C++ vtable dispatch (epic-cc#832).
/// `flash` holds regs proven to carry a vtable (flash) address: `load
/// ptr` results read wholly from vptr fields, closed under all-flash
/// phi/select arms. `mixed` holds partially flash derivations and `gep`
/// over flash regs: unserved shapes every backend panics on. C programs
/// have no nonzero-addend refs, so both sets stay empty there.
#[derive(Clone, Debug, Default)]
pub struct FlashProvenance {
    pub flash: HashSet<String>,
    pub mixed: HashSet<String>,
    /// Integer regs (`{func}::{reg}`) and integer globals (`@{name}`) that
    /// may carry a flash table address laundered through the integer
    /// domain (epic-cc#1284). An `inttoptr` of one would dereference the
    /// table address as RAM, so the backend refuses it.
    pub int_flash: HashSet<String>,
    /// Pointer slots (`{func}::{slot}`) an `inttoptr` produced from a
    /// definitely-flash integer (epic-cc#1346). Each holds a flash
    /// address, so a dereference through it seeds `TBLPTR` from the slot
    /// bytes exactly like a flash-kind pointer slot.
    pub int_flash_ptr: HashSet<String>,
}

/// One ultimate address base behind a pointer value: a global plus a
/// constant byte offset, a reg already proven to hold a flash address, or
/// an untracked runtime value.
enum ProvBase {
    Global(String, u16),
    FlashAddr,
    Unknown,
}

/// Whether global `g`'s bytes `[off, off + n)` are all vptr fields: every
/// byte carries a nonzero-addend ref into a `const` vtable (`_ZTV*`,
/// Itanium ABI prefix), the folded constant GEP of a vptr initializer.
/// The target check is load-bearing: a plain constant GEP initializer
/// (`&arr[2]` in C) folds to the same nonzero-addend shape but names a
/// RAM address, and must never seed flash provenance.
fn vptr_covered(m: &Module, g: &str, off: u16, n: u8) -> bool {
    let is_vtable =
        |t: &str| t.starts_with("_ZTV") && m.globals.iter().any(|x| x.name == t && x.is_const);
    let Some(gl) = m.globals.iter().find(|x| x.name == g) else {
        return false;
    };
    (0..n).all(|i| {
        let pos = off.wrapping_add(u16::from(i)) as usize;
        gl.refs
            .iter()
            .any(|(p, t, add)| *p == pos && *add != 0 && is_vtable(t))
    })
}

/// Flatten a pointer `Val` to its ultimate global bases, following
/// constant `gep` chains and phi/select arms. A reg defined by anything
/// else (a load, call, param, alloca, dynamic `gep` term) is `Unknown`.
/// A reg already in `flash` surfaces as a `FlashAddr` base: dereferencing
/// one is the slot-load shape, not another vptr load.
fn prov_bases(
    defs: &HashMap<String, &Inst>,
    v: &ir::Val,
    k: u16,
    func: &str,
    flash: &HashSet<String>,
) -> Vec<ProvBase> {
    match v {
        ir::Val::Global(g) => vec![ProvBase::Global(g.clone(), k)],
        ir::Val::Const(_) => vec![ProvBase::Unknown],
        ir::Val::Reg(r) => {
            if flash.contains(&ssa_key(func, r)) {
                return vec![ProvBase::FlashAddr];
            }
            let Some(inst) = defs.get(r) else {
                return vec![ProvBase::Unknown];
            };
            match inst {
                Inst::Gep(g) => {
                    if !g.terms.is_empty() {
                        return vec![ProvBase::Unknown];
                    }
                    let nk = k.wrapping_add(g.k);
                    match &g.base {
                        GepBase::Global(n) => vec![ProvBase::Global(n.clone(), nk)],
                        GepBase::Reg(r2) => {
                            prov_bases(defs, &ir::Val::Reg(r2.clone()), nk, func, flash)
                        }
                    }
                }
                Inst::Select(s) if s.ptr => prov_bases(defs, &s.a, k, func, flash)
                    .into_iter()
                    .chain(prov_bases(defs, &s.b, k, func, flash))
                    .collect(),
                Inst::Phi(p) if p.ptr => p
                    .incoming
                    .iter()
                    .flat_map(|(av, _)| prov_bases(defs, av, k, func, flash))
                    .collect(),
                _ => vec![ProvBase::Unknown],
            }
        }
    }
}

/// Classify every `load ptr` result as holding a flash (vptr) address or
/// not, and every phi/select/gep reg deriving from one as flash or
/// mixed. Runs to fixpoint: one pass seeds direct vptr loads, later
/// passes close over arms of newly flash regs.
pub fn flash_provenance(m: &Module) -> FlashProvenance {
    let mut out = FlashProvenance::default();
    loop {
        let mut progressed = false;
        for f in &m.funcs {
            let mut defs: HashMap<String, &Inst> = HashMap::new();
            for b in &f.blocks {
                for i in &b.insts {
                    match i {
                        Inst::Gep(g) => {
                            defs.insert(g.dst.clone(), i);
                        }
                        Inst::Select(s) => {
                            defs.insert(s.dst.clone(), i);
                        }
                        Inst::Phi(p) => {
                            defs.insert(p.dst.clone(), i);
                        }
                        Inst::Load(l) => {
                            defs.insert(l.dst.clone(), i);
                        }
                        _ => {}
                    }
                }
            }
            for b in &f.blocks {
                for i in &b.insts {
                    match i {
                        Inst::Load(l) if l.ptr_ty => {
                            let key = ssa_key(&f.name, &l.dst);
                            if out.flash.contains(&key) || out.mixed.contains(&key) {
                                continue;
                            }
                            let ptr = if let Some(g) = l.ptr.strip_prefix('@') {
                                ir::Val::Global(g.to_string())
                            } else if let Some(r) = l.ptr.strip_prefix('%') {
                                ir::Val::Reg(r.to_string())
                            } else {
                                continue;
                            };
                            let bases = prov_bases(&defs, &ptr, 0, &f.name, &out.flash);
                            if bases.is_empty() {
                                continue;
                            }
                            let n = l.ty.bytes();
                            let (mut covered, mut addrs) = (0usize, 0usize);
                            for bse in &bases {
                                match bse {
                                    ProvBase::FlashAddr => addrs += 1,
                                    ProvBase::Global(g, off) => {
                                        if vptr_covered(m, g, *off, n) {
                                            covered += 1;
                                        }
                                    }
                                    ProvBase::Unknown => {}
                                }
                            }
                            // All bases already flash addresses: this load
                            // dereferences one (the slot-load shape). Its dst
                            // needs no mark: backends route on the ptr reg's
                            // own membership. Anything partly flash-derived
                            // is a shape the runtime sequence does not serve.
                            if addrs == bases.len() {
                                continue;
                            }
                            if covered == bases.len() {
                                out.flash.insert(key);
                                progressed = true;
                            } else if covered > 0 || addrs > 0 {
                                out.mixed.insert(key);
                                progressed = true;
                            }
                        }
                        Inst::Select(s) if s.ptr => {
                            let key = ssa_key(&f.name, &s.dst);
                            if out.flash.contains(&key) || out.mixed.contains(&key) {
                                continue;
                            }
                            let arms = [&s.a, &s.b];
                            let (mut fc, mut mc, mut uc) = (0, 0, 0);
                            for a in arms {
                                match a {
                                    ir::Val::Reg(r) if out.flash.contains(&ssa_key(&f.name, r)) => {
                                        fc += 1;
                                    }
                                    ir::Val::Reg(r) if out.mixed.contains(&ssa_key(&f.name, r)) => {
                                        mc += 1;
                                    }
                                    _ => uc += 1,
                                }
                            }
                            if mc > 0 || (fc > 0 && uc > 0) {
                                out.mixed.insert(key);
                                progressed = true;
                            } else if fc == 2 {
                                out.flash.insert(key);
                                progressed = true;
                            }
                        }
                        Inst::Phi(p) if p.ptr => {
                            let key = ssa_key(&f.name, &p.dst);
                            if out.flash.contains(&key) || out.mixed.contains(&key) {
                                continue;
                            }
                            let (mut fc, mut mc, mut uc) = (0, 0, 0);
                            for (av, _) in &p.incoming {
                                match av {
                                    ir::Val::Reg(r) if out.flash.contains(&ssa_key(&f.name, r)) => {
                                        fc += 1;
                                    }
                                    ir::Val::Reg(r) if out.mixed.contains(&ssa_key(&f.name, r)) => {
                                        mc += 1;
                                    }
                                    _ => uc += 1,
                                }
                            }
                            if mc > 0 || (fc > 0 && uc > 0) {
                                out.mixed.insert(key);
                                progressed = true;
                            } else if fc > 0 && uc == 0 {
                                out.flash.insert(key);
                                progressed = true;
                            }
                        }
                        Inst::Gep(g) => {
                            let base_flash = match &g.base {
                                GepBase::Global(_) => false,
                                GepBase::Reg(r) => {
                                    out.flash.contains(&ssa_key(&f.name, r))
                                        || out.mixed.contains(&ssa_key(&f.name, r))
                                }
                            };
                            if base_flash {
                                let key = ssa_key(&f.name, &g.dst);
                                if out.mixed.insert(key) {
                                    progressed = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

/// Marks flash-walk pointer regs: phi, select, and gep regs whose
/// every base is a flash `const` global (`is_flash_const`), closed over reg
/// arms. A loop walk over a flash string (`%p = phi ptr [@s+1, ..], [%n, ..]`
/// with `%n = gep %p +1`) has no RAM base, so a load through it must read
/// program memory via TBLRD (epic-cc#1091). Greatest fixpoint: a self-advancing
/// cycle stays. A dropped reg that still reaches a flash arm is `mixed`
/// (epic-cc#1099), so loads and stores through it panic. A gep over a walk phi
/// has no slot; isel-pic18 reads it through the phi's slot.
pub fn add_flash_walks(
    prov: &mut FlashProvenance,
    m: &Module,
    is_flash_const: impl Fn(&str) -> bool,
) {
    enum Arm {
        Ok,
        Reg(String),
        Bad,
    }
    let arm = |v: &ir::Val| match v {
        ir::Val::Global(n) if is_flash_const(n) => Arm::Ok,
        ir::Val::Reg(r) => Arm::Reg(r.clone()),
        _ => Arm::Bad,
    };
    for f in &m.funcs {
        let mut cands: Vec<(String, Vec<Arm>)> = Vec::new();
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Phi(p) if p.ptr => cands.push((
                        p.dst.clone(),
                        p.incoming.iter().map(|(v, _)| arm(v)).collect(),
                    )),
                    Inst::Select(s) if s.ptr => {
                        cands.push((s.dst.clone(), vec![arm(&s.a), arm(&s.b)]));
                    }
                    Inst::Gep(g) => {
                        let base = match &g.base {
                            GepBase::Global(n) if is_flash_const(n) => Arm::Ok,
                            GepBase::Global(_) => Arm::Bad,
                            GepBase::Reg(r) => Arm::Reg(r.clone()),
                        };
                        cands.push((g.dst.clone(), vec![base]));
                    }
                    _ => {}
                }
            }
        }
        let mut live: HashSet<String> = cands.iter().map(|(d, _)| d.clone()).collect();
        let mut changed = true;
        while changed {
            changed = false;
            let dead: Vec<String> = cands
                .iter()
                .filter(|(d, arms)| {
                    live.contains(d)
                        && !arms.iter().all(|a| match a {
                            Arm::Ok => true,
                            Arm::Reg(r) => live.contains(r),
                            Arm::Bad => false,
                        })
                })
                .map(|(d, _)| d.clone())
                .collect();
            for d in dead {
                live.remove(&d);
                changed = true;
            }
        }
        // A dead candidate that still reaches a flash arm, directly or through
        // a live, mixed, or flash-tagged reg, holds a flash address on some path
        // and RAM on another: no single address space (epic-cc#1099).
        let mut maybe_flash: HashSet<String> = HashSet::new();
        let mut grew = true;
        while grew {
            grew = false;
            for (d, arms) in &cands {
                if live.contains(d) || maybe_flash.contains(d) {
                    continue;
                }
                let touches = arms.iter().any(|a| match a {
                    Arm::Ok => true,
                    Arm::Reg(r) => {
                        let key = ssa_key(&f.name, r);
                        live.contains(r)
                            || maybe_flash.contains(r)
                            || prov.flash.contains(&key)
                            || prov.mixed.contains(&key)
                    }
                    Arm::Bad => false,
                });
                if touches {
                    maybe_flash.insert(d.clone());
                    grew = true;
                }
            }
        }
        for d in maybe_flash {
            let key = ssa_key(&f.name, &d);
            // flash_provenance already tagged this vptr key flash; keep it.
            if !prov.flash.contains(&key) {
                prov.mixed.insert(key);
            }
        }
        for d in live {
            let key = ssa_key(&f.name, &d);
            prov.mixed.remove(&key);
            prov.flash.insert(key);
        }
    }
}

/// Walk marks for the PIC14 family: `add_flash_walks` over the consts that
/// keep no RAM copy (not in `addrs`, not `staged`) and are not C++ vtables.
/// Those consts' addresses are flash labels, which a pointer slot hands to
/// FSR/INDF as if RAM (epic-cc#1197, epic-cc#1203).
pub fn pic14_flash_walks(
    m: &Module,
    addrs: &HashMap<String, u16>,
    staged: &HashSet<String>,
) -> FlashProvenance {
    let flash_consts: HashSet<&str> = m
        .globals
        .iter()
        .filter(|g| {
            g.is_const
                && !g.name.starts_with("_ZTV")
                && !staged.contains(&g.name)
                && !addrs.contains_key(&g.name)
        })
        .map(|g| g.name.as_str())
        .collect();
    let mut walks = FlashProvenance::default();
    add_flash_walks(&mut walks, m, |n| flash_consts.contains(n));
    walks
}

/// Refusal text when slot `slot` of `func` holds a flash-walk address (or a
/// pointer joining flash and RAM): a load, store, or memcpy through it would
/// read or write RAM at a flash address, and the PIC14 family has no
/// variable-index flash reader. `issue` is the backend's tracking ticket.
pub fn flash_walk_refusal(
    walks: &FlashProvenance,
    func: &str,
    slot: &str,
    issue: &str,
) -> Option<String> {
    let key = ssa_key(func, slot);
    if walks.flash.contains(&key) {
        Some(format!(
            "pointer access through flash walk %{slot} (flash walks read program memory, PIC18-only, epic-cc#1091, epic-cc#{issue})"
        ))
    } else if walks.mixed.contains(&key) {
        Some(format!(
            "pointer access through maybe-flash %{slot} (a pointer joining flash and RAM, PIC18-only, epic-cc#1099, epic-cc#{issue})"
        ))
    } else {
        None
    }
}

/// A store destination behind the space proof: a precise global, an
/// object-local slot (writes stay inside the object), or an unknown
/// target (demote taken globals).
#[derive(Clone, Debug)]
enum DstSpace {
    Global(String),
    Object,
    Unknown,
}

/// A writer into a pointer slot, classified for the flash-kind proof below.
/// `Src` grounds flash: a flash const global or a walk reg already proven
/// flash (itself grounded in consts). `Null` is compatible but grounds
/// nothing, so a slot holding only null stays RAM. `Node` delegates to
/// another candidate. Anything else (RAM values, unknown callers) is `Bad`
/// and prunes the slot to RAM.
#[derive(Clone, Debug)]
enum SlotWriter {
    Src,
    Null,
    Node(String),
    Bad,
}

/// Static address space of every pointer slot (epic-cc#1119): params and
/// phi/select/gep/load-ptr dsts keyed `{func}::{slot}`, globals as
/// `@{global}`. A slot is flash-kind when every address it can hold is a
/// flash table address or null, so loads through it must read program
/// memory. Greatest fixpoint over writers, then an ungrounded prune: a
/// cycle of slots passing each other addresses with no const source (two
/// functions forwarding a RAM pointer back and forth) stays RAM.
/// `is_flash_const` names the consts resident in flash, `prov` the
/// already-proven flash walks, `resolved` the pointer chains for
/// load-source slots. Const globals are never slots: their bytes are
/// tables, read through the const paths instead.
pub fn flash_slot_spaces(
    m: &Module,
    resolved: &PtrResolution,
    prov: &FlashProvenance,
    is_flash_const: impl Fn(&str) -> bool,
) -> HashSet<String> {
    flash_slot_spaces_ex(m, resolved, prov, is_flash_const).0
}

/// `flash_slot_spaces` plus the maybe-flash slots: those that can hold a
/// flash table address on some path and a RAM (or unknown) one on another,
/// so no single space applies (epic-cc#1284). Loads of such a slot must
/// never be dereferenced as RAM. The third set names the functions whose
/// returns need the 3-byte fat form: any returned value may carry flash,
/// so every pointer return there writes the tag (epic-cc#1306).
pub fn flash_slot_spaces_ex(
    m: &Module,
    resolved: &PtrResolution,
    prov: &FlashProvenance,
    is_flash_const: impl Fn(&str) -> bool,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let is_func = |n: &str| m.funcs.iter().any(|f| f.name == n);
    // Candidate slots: plain ptr params plus every reg that can hold an
    // address (ptr phis/selects, geps, ptr loads, freezes, call results
    // used in pointer position). byval/sret params hold caller copies,
    // integer-only call results and inttoptr/va_arg/alloca slots hold
    // RAM or runtime integers, so none of those are candidates.
    let mut cands: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for p in &f.params {
            if p.ptr && p.byval.is_none() && !p.sret {
                cands.insert(ssa_key(&f.name, &p.name));
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Phi(p) if p.ptr => {
                        cands.insert(ssa_key(&f.name, &p.dst));
                    }
                    Inst::Select(s) if s.ptr => {
                        cands.insert(ssa_key(&f.name, &s.dst));
                    }
                    Inst::Gep(g) => {
                        cands.insert(ssa_key(&f.name, &g.dst));
                    }
                    Inst::Load(l) if l.ptr_ty => {
                        cands.insert(ssa_key(&f.name, &l.dst));
                    }
                    Inst::Freeze(fr) => {
                        cands.insert(ssa_key(&f.name, &fr.dst));
                    }
                    _ => {}
                }
            }
        }
    }
    for g in &m.globals {
        if !g.is_const {
            cands.insert(format!("@{}", g.name));
        }
    }
    // A call result used in pointer position holds an address on return
    // (epic-cc#1306): a GEP base, a load/store address operand, a memcpy
    // side, a pointer call argument, a returned value, or a ptr
    // phi/select/freeze arm. Integer-only results stay out, so integer
    // math never widens to a tagged slot.
    for f in &m.funcs {
        let mut ptr_used: HashSet<String> = HashSet::new();
        let mark_many = |vs: Vec<&ir::Val>, set: &mut HashSet<String>| {
            for v in vs {
                if let ir::Val::Reg(r) = v {
                    set.insert(r.clone());
                }
            }
        };
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Gep(g) => {
                        if let GepBase::Reg(r) = &g.base {
                            ptr_used.insert(r.clone());
                        }
                    }
                    Inst::Load(l) => {
                        if let Some(r) = l.ptr.strip_prefix('%') {
                            ptr_used.insert(r.to_string());
                        }
                    }
                    Inst::Store(s) => {
                        if let Some(r) = s.ptr.strip_prefix('%') {
                            ptr_used.insert(r.to_string());
                        }
                    }
                    Inst::Memcpy(mc) => {
                        mark_many(vec![&mc.src, &mc.dst], &mut ptr_used);
                    }
                    Inst::Call(c) => {
                        let vs: Vec<&ir::Val> = c
                            .args
                            .iter()
                            .filter(|a| a.ty.is_none())
                            .map(|a| &a.val)
                            .collect();
                        mark_many(vs, &mut ptr_used);
                    }
                    Inst::Ret(Some((_, v)), _) => {
                        mark_many(vec![v], &mut ptr_used);
                    }
                    Inst::Phi(p) if p.ptr => {
                        let vs: Vec<&ir::Val> = p.incoming.iter().map(|(v, _)| v).collect();
                        mark_many(vs, &mut ptr_used);
                    }
                    Inst::Select(s) if s.ptr => {
                        mark_many(vec![&s.a, &s.b], &mut ptr_used);
                    }
                    Inst::Freeze(fr) => {
                        mark_many(vec![&fr.val], &mut ptr_used);
                    }
                    _ => {}
                }
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(c) = i {
                    if let Some(d) = &c.dst {
                        if ptr_used.contains(d) {
                            cands.insert(ssa_key(&f.name, d));
                        }
                    }
                }
            }
        }
    }
    // Slots whose bytes ARE an object, never an address: stores through
    // pointers derived from them stay object-local and never touch a
    // global slot. Allocas and byval params qualify; every other slot
    // with address bytes may point anywhere.
    let mut objects: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Alloca(a) = i {
                    objects.insert(ssa_key(&f.name, &a.dst));
                }
            }
        }
        for p in &f.params {
            if p.byval.is_some() {
                objects.insert(ssa_key(&f.name, &p.name));
            }
        }
    }
    // Globals whose address is taken as a value anywhere (stored,
    // passed, returned, or selected): only these can be overwritten
    // through an unknown pointer, so only these demote on one.
    let mut taken: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Store(s) => {
                        taken.extend(global_fan_in(m, resolved, &f.name, &s.val));
                    }
                    Inst::Call(c) => {
                        for arg in &c.args {
                            taken.extend(global_fan_in(m, resolved, &f.name, &arg.val));
                        }
                    }
                    Inst::Ret(Some((_, v)), _) => {
                        taken.extend(global_fan_in(m, resolved, &f.name, v));
                    }
                    Inst::Select(s) if s.ptr => {
                        taken.extend(global_fan_in(m, resolved, &f.name, &s.a));
                        taken.extend(global_fan_in(m, resolved, &f.name, &s.b));
                    }
                    Inst::Phi(p) if p.ptr => {
                        for (v, _) in &p.incoming {
                            taken.extend(global_fan_in(m, resolved, &f.name, v));
                        }
                    }
                    Inst::Memcpy(mc) => {
                        taken.extend(global_fan_in(m, resolved, &f.name, &mc.src));
                        taken.extend(global_fan_in(m, resolved, &f.name, &mc.dst));
                    }
                    Inst::Freeze(fr) => {
                        taken.extend(global_fan_in(m, resolved, &f.name, &fr.val));
                    }
                    _ => {}
                }
            }
        }
    }
    // A value offered to a slot: flash consts and proven walks ground it,
    // null is compatible, candidate regs delegate, everything else prunes.
    // Function addresses are RAM-kind: calls through them copy bytes, but
    // a dereference must never TBLRD code space (epic-cc#1119).
    let val_writer = |v: &ir::Val, func: &str| -> SlotWriter {
        match v {
            ir::Val::Global(n) => {
                if is_func(n) || !is_flash_const(n) {
                    SlotWriter::Bad
                } else {
                    SlotWriter::Src
                }
            }
            ir::Val::Reg(r) => {
                let key = ssa_key(func, r);
                if prov.flash.contains(&key) {
                    SlotWriter::Src
                } else if cands.contains(&key) {
                    SlotWriter::Node(key)
                } else {
                    SlotWriter::Bad
                }
            }
            ir::Val::Const(0) => SlotWriter::Null,
            ir::Val::Const(_) => SlotWriter::Bad,
        }
    };
    // Indirect-call targets gain an unknown caller: their params can
    // receive any address, so they prune to RAM.
    let mut indirect_targets: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(c) = i {
                    if !c.callees.is_empty() {
                        indirect_targets.extend(c.callees.iter().cloned());
                    }
                }
            }
        }
    }
    let mut writers: HashMap<String, Vec<SlotWriter>> = HashMap::new();
    let mut push = |key: String, w: SlotWriter| {
        writers.entry(key).or_default().push(w);
    };
    // An unknown-target write may overwrite any taken global.
    let mut unknown_write = false;
    // Classify a store destination: a precise global target, an
    // object-local slot (alloca/byval writes stay inside the object),
    // or an unknown target. Literal addresses name SFR/MMIO, never a
    // global slot: user code cannot know our GPR layout.
    let dst_space = |func: &str, ptr: &str| -> DstSpace {
        if let Some(g) = ptr.strip_prefix('@') {
            return DstSpace::Global(g.to_string());
        }
        if ptr.starts_with("0x") {
            return DstSpace::Object;
        }
        let Some(r) = ptr.strip_prefix('%') else {
            return DstSpace::Unknown;
        };
        match resolved.get(&ssa_key(func, r)) {
            Some((Base::Global(g), _, _)) => DstSpace::Global(g.clone()),
            Some((Base::Slot(sname, _), _, _)) => {
                if objects.contains(&ssa_key(func, sname)) {
                    DstSpace::Object
                } else {
                    DstSpace::Unknown
                }
            }
            None => DstSpace::Unknown,
        }
    };
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Phi(p) if p.ptr => {
                        let key = ssa_key(&f.name, &p.dst);
                        for (v, _) in &p.incoming {
                            push(key.clone(), val_writer(v, &f.name));
                        }
                    }
                    Inst::Select(s) if s.ptr => {
                        let key = ssa_key(&f.name, &s.dst);
                        push(key.clone(), val_writer(&s.a, &f.name));
                        push(key, val_writer(&s.b, &f.name));
                    }
                    Inst::Gep(g) => {
                        let key = ssa_key(&f.name, &g.dst);
                        match &g.base {
                            GepBase::Global(n) => {
                                if is_func(n) || !is_flash_const(n) {
                                    push(key, SlotWriter::Bad);
                                } else {
                                    push(key, SlotWriter::Src);
                                }
                            }
                            GepBase::Reg(r) => {
                                push(key, val_writer(&ir::Val::Reg(r.clone()), &f.name));
                            }
                        }
                    }
                    Inst::Freeze(fr) => {
                        // A freeze copies its operand whole, tag included.
                        push(ssa_key(&f.name, &fr.dst), val_writer(&fr.val, &f.name));
                    }
                    Inst::Load(l) if l.ptr_ty => {
                        let key = ssa_key(&f.name, &l.dst);
                        if let Some(g) = l.ptr.strip_prefix('@') {
                            if let Some(gl) = m.globals.iter().find(|x| x.name == g) {
                                if gl.is_const {
                                    // Bytes come from the flash table: every
                                    // byte must be zero or covered by a ref
                                    // to a flash const, else RAM-kind.
                                    let mut covered = vec![false; gl.bytes.len()];
                                    let mut ok = true;
                                    for (pos, t, _) in &gl.refs {
                                        if is_func(t) || !is_flash_const(t) {
                                            ok = false;
                                            break;
                                        }
                                        if *pos < covered.len() {
                                            covered[*pos] = true;
                                        }
                                    }
                                    if ok
                                        && gl.bytes.iter().enumerate().all(|(n, b)| {
                                            *b == 0 || covered.get(n).copied().unwrap_or(false)
                                        })
                                    {
                                        push(key, SlotWriter::Src);
                                    } else {
                                        push(key, SlotWriter::Bad);
                                    }
                                } else {
                                    push(key, SlotWriter::Node(format!("@{g}")));
                                }
                            } else {
                                push(key, SlotWriter::Bad);
                            }
                        } else if let Some(r) = l.ptr.strip_prefix('%') {
                            match resolved.get(&ssa_key(&f.name, r)) {
                                Some((Base::Slot(sname, _), _, _)) => {
                                    let skey = ssa_key(&f.name, sname);
                                    if cands.contains(&skey) {
                                        push(key, SlotWriter::Node(skey));
                                    } else {
                                        push(key, SlotWriter::Bad);
                                    }
                                }
                                _ => push(key, SlotWriter::Bad),
                            }
                        } else {
                            push(key, SlotWriter::Bad);
                        }
                    }
                    Inst::Store(s) => {
                        // A partial or wide write is not an address write.
                        // Indirect targets resolving to a global write it
                        // precisely; object-local and unknown targets are
                        // classified by `dst_space`.
                        match dst_space(&f.name, &s.ptr) {
                            DstSpace::Global(g) => {
                                if m.globals.iter().any(|x| x.name == g && !x.is_const) {
                                    if s.ty.bytes() == 2 {
                                        push(format!("@{g}"), val_writer(&s.val, &f.name));
                                    } else {
                                        push(format!("@{g}"), SlotWriter::Bad);
                                    }
                                }
                            }
                            DstSpace::Object => {}
                            DstSpace::Unknown => {
                                unknown_write = true;
                            }
                        }
                    }
                    Inst::Memcpy(mc) => {
                        // A copy into a slot overwrites its bytes with
                        // unknown ones: precise globals go Bad, unknown
                        // targets demote taken globals.
                        let dst = |v: &ir::Val| match v {
                            ir::Val::Global(g) => DstSpace::Global(g.clone()),
                            ir::Val::Reg(r) => dst_space(&f.name, &format!("%{r}")),
                            ir::Val::Const(_) => DstSpace::Unknown,
                        };
                        match dst(&mc.dst) {
                            DstSpace::Global(g) => {
                                if m.globals.iter().any(|x| x.name == g && !x.is_const) {
                                    push(format!("@{g}"), SlotWriter::Bad);
                                }
                            }
                            DstSpace::Object => {}
                            DstSpace::Unknown => {
                                unknown_write = true;
                            }
                        }
                    }
                    Inst::Asm(a) => {
                        // Only outputs (constraints starting with `=` or
                        // `+`) write; reads never touch a slot. Blanket
                        // clobbers demote taken globals.
                        if a.clobbers_memory {
                            unknown_write = true;
                        }
                        for op in &a.operands {
                            if !(op.constraint.starts_with('=') || op.constraint.starts_with('+')) {
                                continue;
                            }
                            match dst_space(&f.name, &op.ptr) {
                                DstSpace::Global(g) => {
                                    if m.globals.iter().any(|x| x.name == g && !x.is_const) {
                                        push(format!("@{g}"), SlotWriter::Bad);
                                    }
                                }
                                DstSpace::Object => {}
                                DstSpace::Unknown => {
                                    unknown_write = true;
                                }
                            }
                        }
                    }
                    Inst::Call(c) => {
                        if c.callees.is_empty() {
                            if let Some(callee) = m.funcs.iter().find(|x| x.name == c.func) {
                                for (p, arg) in callee.params.iter().zip(c.args.iter()) {
                                    if !(p.ptr && p.byval.is_none() && !p.sret) {
                                        continue;
                                    }
                                    if arg.byval.is_some() || arg.sret {
                                        push(ssa_key(&c.func, &p.name), SlotWriter::Bad);
                                    } else {
                                        push(
                                            ssa_key(&c.func, &p.name),
                                            val_writer(&arg.val, &f.name),
                                        );
                                    }
                                }
                                if c.args.len() < callee.params.len() {
                                    for p in &callee.params[c.args.len()..] {
                                        if p.ptr && p.byval.is_none() && !p.sret {
                                            push(ssa_key(&c.func, &p.name), SlotWriter::Bad);
                                        }
                                    }
                                }
                                // A call result holds what the callee
                                // returns: each returned value writes the
                                // dst the same way an argument writes a
                                // param (epic-cc#1306). Unknown callees
                                // prune to RAM; non-pointer dsts never
                                // became candidates, so gating on `cands`
                                // keeps integer math out of the sets.
                                if let Some(d) = &c.dst {
                                    let key = ssa_key(&f.name, d);
                                    if cands.contains(&key) {
                                        for b in &callee.blocks {
                                            for i in &b.insts {
                                                if let Inst::Ret(Some((_, v)), _) = i {
                                                    push(key.clone(), val_writer(v, &callee.name));
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if let Some(d) = &c.dst {
                                let key = ssa_key(&f.name, d);
                                if cands.contains(&key) {
                                    push(key, SlotWriter::Bad);
                                }
                            }
                        } else if let Some(d) = &c.dst {
                            // An indirect site may reach any candidate: the
                            // dst joins every candidate's returns.
                            let key = ssa_key(&f.name, d);
                            if cands.contains(&key) {
                                for cand in &c.callees {
                                    if let Some(callee) = m.funcs.iter().find(|x| x.name == *cand) {
                                        for b in &callee.blocks {
                                            for i in &b.insts {
                                                if let Inst::Ret(Some((_, v)), _) = i {
                                                    push(key.clone(), val_writer(v, &callee.name));
                                                }
                                            }
                                        }
                                    } else {
                                        push(key.clone(), SlotWriter::Bad);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // Globals start with their initializer as a writer: zero bytes are
    // null, refs to flash consts are flash (and ground it), anything
    // else is RAM.
    for g in &m.globals {
        if g.is_const {
            continue;
        }
        let key = format!("@{}", g.name);
        let mut covered = vec![false; g.bytes.len()];
        let mut ok = true;
        for (pos, t, _) in &g.refs {
            if is_func(t) || !is_flash_const(t) {
                ok = false;
                break;
            }
            if *pos < covered.len() {
                covered[*pos] = true;
            }
        }
        if ok
            && g.bytes
                .iter()
                .enumerate()
                .all(|(n, b)| *b == 0 || covered.get(n).copied().unwrap_or(false))
        {
            if g.refs.is_empty() {
                push(key, SlotWriter::Null);
            } else {
                push(key, SlotWriter::Src);
            }
        } else {
            push(key, SlotWriter::Bad);
        }
    }
    // Params of indirectly-called functions gain the unknown caller.
    for f in &m.funcs {
        if !indirect_targets.contains(&f.name) {
            continue;
        }
        for p in &f.params {
            if p.ptr && p.byval.is_none() && !p.sret {
                push(ssa_key(&f.name, &p.name), SlotWriter::Bad);
            }
        }
    }
    // An unknown-target write may overwrite any taken global: demote
    // them all. Globals never taken as values cannot be reached this
    // way, so purely dereferenced pointer globals keep their proof.
    if unknown_write {
        for t in &taken {
            writers
                .entry(format!("@{t}"))
                .or_default()
                .push(SlotWriter::Bad);
        }
    }
    // Greatest fixpoint: drop candidates with a RAM writer, then drop
    // cycles no const grounds.
    let mut live: HashSet<String> = cands;
    loop {
        let dead: Vec<String> = writers
            .iter()
            .filter(|(k, ws)| {
                live.contains(*k)
                    && ws.iter().any(|w| match w {
                        SlotWriter::Bad => true,
                        SlotWriter::Node(n) => !live.contains(n),
                        SlotWriter::Src | SlotWriter::Null => false,
                    })
            })
            .map(|(k, _)| k.clone())
            .collect();
        if dead.is_empty() {
            break;
        }
        for d in dead {
            live.remove(&d);
        }
    }
    let mut grounded: HashSet<String> = HashSet::new();
    let mut grew = true;
    while grew {
        grew = false;
        for (k, ws) in &writers {
            if !live.contains(k) || grounded.contains(k) {
                continue;
            }
            let seeded = ws.iter().any(|w| match w {
                SlotWriter::Src => true,
                SlotWriter::Node(n) => grounded.contains(n),
                SlotWriter::Null | SlotWriter::Bad => false,
            });
            if seeded {
                grounded.insert(k.clone());
                grew = true;
            }
        }
    }
    let flash: HashSet<String> = live.intersection(&grounded).cloned().collect();
    // Least fixpoint of "may hold a flash address": a flash writer, or a
    // delegate that may. Whatever is not flash-kind is maybe-flash.
    let mut may: HashSet<String> = writers
        .iter()
        .filter(|(_, ws)| ws.iter().any(|w| matches!(w, SlotWriter::Src)))
        .map(|(k, _)| k.clone())
        .collect();
    let mut grew = true;
    while grew {
        grew = false;
        for (k, ws) in &writers {
            if may.contains(k) {
                continue;
            }
            if ws
                .iter()
                .any(|w| matches!(w, SlotWriter::Node(n) if may.contains(n)))
            {
                may.insert(k.clone());
                grew = true;
            }
        }
    }
    let mixed: HashSet<String> = may.difference(&flash).cloned().collect();
    // Functions whose returns need the 3-byte fat form (epic-cc#1306):
    // any returned value may carry flash, so every pointer return of
    // such a function writes address bytes plus the tag. Seeding on a
    // mixed return alone would leave a RAM return of the same function
    // writing two bytes behind a stale tag, so the seed covers every
    // may-flash return and every return of an indirectly co-reachable
    // function shares it.
    let val_may_flash = |v: &ir::Val, func: &str| -> bool {
        match v {
            ir::Val::Global(n) => !is_func(n) && is_flash_const(n),
            ir::Val::Reg(r) => {
                let key = ssa_key(func, r);
                may.contains(&key) || prov.flash.contains(&key)
            }
            ir::Val::Const(_) => false,
        }
    };
    let mut ret_fat: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        let fat = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Ret(Some((_, v)), _) if val_may_flash(v, &f.name)));
        if fat {
            ret_fat.insert(f.name.clone());
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Call(c) = i {
                        if c.dst.is_none() || c.callees.is_empty() {
                            continue;
                        }
                        if c.callees.iter().any(|t| ret_fat.contains(t)) {
                            for t in &c.callees {
                                if ret_fat.insert(t.clone()) {
                                    grew = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (flash, mixed, ret_fat)
}

/// Mark the results of pointer loads from maybe-flash slots as mixed
/// (they carry flash or RAM per path, epic-cc#1284), propagate through
/// pointer derivations, and compute the integer-domain flash taint. Call
/// after `flash_slot_spaces_ex`; `slot_flash` and `slot_mixed` are its
/// two sets.
pub fn add_flash_laundering(
    prov: &mut FlashProvenance,
    m: &Module,
    resolved: &PtrResolution,
    slot_flash: &HashSet<String>,
    slot_mixed: &HashSet<String>,
    is_flash_const: impl Fn(&str) -> bool,
) {
    // Slot key a load/store pointer operand names, if any.
    let slot_of = |func: &str, ptr: &str| -> Option<String> {
        if let Some(g) = ptr.strip_prefix('@') {
            return Some(format!("@{g}"));
        }
        let r = ptr.strip_prefix('%')?;
        match resolved.get(&ssa_key(func, r)) {
            Some((Base::Global(g), _, _)) => Some(format!("@{g}")),
            Some((Base::Slot(sname, _), _, _)) => Some(ssa_key(func, sname)),
            None => None,
        }
    };
    // Pointer loads from a maybe-flash slot, then derivations of them.
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Load(l) = i {
                    if l.ptr_ty {
                        if let Some(k) = slot_of(&f.name, &l.ptr) {
                            if slot_mixed.contains(&k) {
                                prov.mixed.insert(ssa_key(&f.name, &l.dst));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    let (dst, srcs): (&String, Vec<&ir::Val>) = match i {
                        Inst::Gep(g) => match &g.base {
                            GepBase::Reg(r) => {
                                let key = ssa_key(&f.name, r);
                                if prov.mixed.contains(&key)
                                    && prov.mixed.insert(ssa_key(&f.name, &g.dst))
                                {
                                    grew = true;
                                }
                                continue;
                            }
                            GepBase::Global(_) => continue,
                        },
                        Inst::Phi(p) if p.ptr => {
                            (&p.dst, p.incoming.iter().map(|(v, _)| v).collect())
                        }
                        Inst::Select(s) if s.ptr => (&s.dst, vec![&s.a, &s.b]),
                        Inst::Freeze(fr) => (&fr.dst, vec![&fr.val]),
                        _ => continue,
                    };
                    let hit = srcs.iter().any(|v| match v {
                        ir::Val::Reg(r) => prov.mixed.contains(&ssa_key(&f.name, r)),
                        _ => false,
                    });
                    if hit && prov.mixed.insert(ssa_key(&f.name, dst)) {
                        grew = true;
                    }
                }
            }
        }
    }
    // Integer-domain taint: a flash address copied as integer data.
    // Registers are tracked by name; memory by (slot, byte offset) so a
    // struct holding a flash pointer next to an integer address field only
    // taints the pointer's bytes. A dynamic offset widens to the whole slot.
    #[derive(Default)]
    struct MemTaint {
        bytes: HashSet<(String, u32)>,
        wide: HashSet<String>,
        /// A tainted store went through a pointer we could not resolve to
        /// a slot: any integer load may see it.
        all: bool,
    }
    /// Constant offset `k`; `g > 0` means a runtime index in steps of
    /// multiples of `g` (the gcd of the term scales) on top of `k`.
    type Off = (u32, u32);
    impl MemTaint {
        fn mark(&mut self, slot: &str, off: Off, n: u32) {
            if off.1 > 0 {
                self.wide.insert(slot.to_string());
                return;
            }
            for i in 0..n.max(1) {
                self.bytes.insert((slot.to_string(), off.0 + i));
            }
        }
        fn query(&self, slot: &str, off: Off, n: u32) -> bool {
            if self.all || self.wide.contains(slot) {
                return true;
            }
            let (k, g) = off;
            self.bytes.iter().any(|(s, t)| {
                s == slot
                    && (0..n.max(1)).any(|j| {
                        if g == 0 {
                            *t == k + j
                        } else {
                            (i64::from(*t) - i64::from(k + j)).rem_euclid(i64::from(g)) == 0
                        }
                    })
            })
        }
        fn size(&self) -> usize {
            self.bytes.len() + self.wide.len() + usize::from(self.all)
        }
    }
    // Memory location a pointer operand names: slot key plus constant byte
    // offset (`None` when a runtime term makes it dynamic).
    // Slots whose bytes are an object (alloca, byval param); every other
    // slot with address bytes may point anywhere.
    let mut objects: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Alloca(a) = i {
                    objects.insert(ssa_key(&f.name, &a.dst));
                }
            }
        }
        for p in &f.params {
            if p.byval.is_some() {
                objects.insert(ssa_key(&f.name, &p.name));
            }
        }
    }
    let stride = |terms: &[(u16, String)]| -> u32 {
        fn gcd(a: u32, b: u32) -> u32 {
            if b == 0 {
                a
            } else {
                gcd(b, a % b)
            }
        }
        terms
            .iter()
            .fold(0u32, |g, (sc, _)| gcd(g, u32::from(*sc)))
            .max(u32::from(!terms.is_empty()))
    };
    // The bool is true when the pointer did not resolve to a known slot.
    let loc = |func: &str, ptr: &str| -> Option<(String, Off, bool)> {
        if let Some(g) = ptr.strip_prefix('@') {
            return Some((format!("@{g}"), (0, 0), false));
        }
        let r = ptr.strip_prefix('%')?;
        match resolved.get(&ssa_key(func, r)) {
            Some((Base::Global(g), k, terms)) => {
                Some((format!("@{g}"), (u32::from(*k), stride(terms)), false))
            }
            // A slot holding a pointer value (param, loaded pointer): the
            // pointee may alias any slot, so it is an unknown location.
            Some((Base::Slot(sname, _), k, terms)) => {
                let key = ssa_key(func, sname);
                let unknown = !objects.contains(&key);
                Some((key, (u32::from(*k), stride(terms)), unknown))
            }
            None => Some((ssa_key(func, r), (0, 0), true)),
        }
    };
    let val_loc = |func: &str, v: &ir::Val| -> Option<(String, Off, bool)> {
        match v {
            ir::Val::Global(g) => Some((format!("@{g}"), (0, 0), false)),
            ir::Val::Reg(r) => loc(func, &format!("%{r}")),
            ir::Val::Const(_) => None,
        }
    };
    // Integer-domain taint as caller-independent taint plus the callee
    // params the value may depend on. `indep` covers flash constants,
    // provenance sets, flash-kind slots, and tainted memory (which
    // persists across calls, so loads are caller-independent). `deps`
    // holds param keys (`{func}::{param}`); a call result substitutes
    // each callee param with the site's actual argument state, so one
    // tainted caller no longer taints every caller's result (epic-cc#1324).
    #[derive(Clone, Default)]
    struct TState {
        indep: bool,
        deps: HashSet<String>,
    }
    let ts_bool = |s: &TState| -> bool { s.indep || !s.deps.is_empty() };
    let ts_union = |a: &mut TState, b: &TState| {
        a.indep |= b.indep;
        a.deps.extend(b.deps.iter().cloned());
    };
    let mut t: HashMap<String, TState> = HashMap::new();
    // Address fragments: single bytes (or scaled bytes) of a tainted
    // address, via `and 0xFF` or `lshr 8`. One fragment cannot name a
    // 16-bit table address, but `or`/`add` of two rebuilds it, so the
    // Bin arm re-taints reassembled pairs. Register-only: a fragment
    // parked in memory goes clean (residual gap). Calls substitute
    // fragment states the same way `t` does, so reassembly across a
    // call still re-taints.
    let mut frag: HashMap<String, TState> = HashMap::new();
    // Callee params (`{func}::{param}`) with a tainted actual at some
    // call site. Inside-callee uses stay refused existentially, while
    // each call result is gated on its own site's arguments.
    let mut ptaint: HashSet<String> = HashSet::new();
    let mut pfrag: HashSet<String> = HashSet::new();
    // Every param key, so operand states always carry their own dep.
    let mut param_keys: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for p in &f.params {
            param_keys.insert(ssa_key(&f.name, &p.name));
        }
    }
    let mut mt = MemTaint::default();
    // Flash references in a global's initializer are flash addresses.
    for g in &m.globals {
        for (pos, target, _) in &g.refs {
            if is_flash_const(target) {
                mt.mark(&format!("@{}", g.name), (*pos as u32, 0), 2);
            }
        }
    }
    let tp = |t: &HashMap<String, TState>,
              param_keys: &HashSet<String>,
              prov: &FlashProvenance,
              func: &str,
              v: &ir::Val|
     -> TState {
        match v {
            ir::Val::Global(n) if is_flash_const(n) => TState {
                indep: true,
                deps: HashSet::new(),
            },
            ir::Val::Global(_) => TState::default(),
            ir::Val::Reg(r) => {
                let k = ssa_key(func, r);
                let mut s = t.get(&k).cloned().unwrap_or_default();
                if param_keys.contains(&k) {
                    s.deps.insert(k.clone());
                }
                let indep = prov.flash.contains(&k)
                    || prov.mixed.contains(&k)
                    || slot_flash.contains(&k)
                    || slot_mixed.contains(&k)
                    || match resolved.get(&k) {
                        Some((Base::Global(g), _, _)) => is_flash_const(g),
                        Some((Base::Slot(sn, _), _, _)) => {
                            let sk = ssa_key(func, sn);
                            slot_flash.contains(&sk) || slot_mixed.contains(&sk)
                        }
                        None => false,
                    };
                s.indep |= indep;
                s
            }
            ir::Val::Const(_) => TState::default(),
        }
    };
    let tv = |t: &HashMap<String, TState>,
              param_keys: &HashSet<String>,
              prov: &FlashProvenance,
              func: &str,
              v: &ir::Val|
     -> bool { ts_bool(&tp(t, param_keys, prov, func, v)) };
    let fp = |frag: &HashMap<String, TState>,
              param_keys: &HashSet<String>,
              func: &str,
              v: &ir::Val|
     -> TState {
        match v {
            ir::Val::Reg(r) => {
                let k = ssa_key(func, r);
                let mut s = frag.get(&k).cloned().unwrap_or_default();
                if param_keys.contains(&k) {
                    s.deps.insert(k);
                }
                s
            }
            _ => TState::default(),
        }
    };
    let fv = |frag: &HashMap<String, TState>,
              param_keys: &HashSet<String>,
              func: &str,
              v: &ir::Val|
     -> bool { ts_bool(&fp(frag, param_keys, func, v)) };
    // Whether a binary op over a tainted operand can still be an address.
    let bin_taints = |x: &ir::Bin, ta: bool, tb: bool| -> bool {
        if !(ta || tb) {
            return false;
        }
        let small = |v: &ir::Val| matches!(v, ir::Val::Const(c) if (0..=0xFF).contains(c));
        match x.op {
            // A difference of two addresses is a length.
            ir::BinOp::Sub => !(ta && tb),
            // Masking to a byte leaves a fragment, not an address; the Bin
            // arm tracks it so a later `or`/`add` reassembly re-taints.
            ir::BinOp::And => !(small(&x.a) || small(&x.b)),
            // Shifting right by a byte or more drops the address the same way.
            ir::BinOp::LShr | ir::BinOp::AShr => !matches!(&x.b, ir::Val::Const(c) if *c >= 8),
            _ => true,
        }
    };
    // Sizes of every growing set: `indep` flags, param deps, merged call
    // taints, and tainted memory bytes. All grow monotonically, so equal
    // sums mean the fixpoint is reached.
    let state_sizes = |t: &HashMap<String, TState>,
                       frag: &HashMap<String, TState>,
                       ptaint: &HashSet<String>,
                       pfrag: &HashSet<String>,
                       mt: &MemTaint|
     -> usize {
        t.values()
            .map(|s| s.deps.len() + usize::from(s.indep))
            .sum::<usize>()
            + frag
                .values()
                .map(|s| s.deps.len() + usize::from(s.indep))
                .sum::<usize>()
            + ptaint.len()
            + pfrag.len()
            + mt.size()
    };
    loop {
        let before = state_sizes(&t, &frag, &ptaint, &pfrag, &mt);
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    let mut add: Vec<(String, TState)> = Vec::new();
                    let mut add_frag: Vec<(String, TState)> = Vec::new();
                    match i {
                        Inst::Zext(z) if tv(&t, &param_keys, prov, &f.name, &z.val) => add.push((
                            ssa_key(&f.name, &z.dst),
                            tp(&t, &param_keys, prov, &f.name, &z.val),
                        )),
                        Inst::Sext(z) if tv(&t, &param_keys, prov, &f.name, &z.val) => add.push((
                            ssa_key(&f.name, &z.dst),
                            tp(&t, &param_keys, prov, &f.name, &z.val),
                        )),
                        Inst::Trunc(z) if tv(&t, &param_keys, prov, &f.name, &z.val) => add.push((
                            ssa_key(&f.name, &z.dst),
                            tp(&t, &param_keys, prov, &f.name, &z.val),
                        )),
                        Inst::Freeze(z) if tv(&t, &param_keys, prov, &f.name, &z.val) => {
                            add.push((
                                ssa_key(&f.name, &z.dst),
                                tp(&t, &param_keys, prov, &f.name, &z.val),
                            ))
                        }
                        Inst::Zext(z) if fv(&frag, &param_keys, &f.name, &z.val) => {
                            // A cast of one address byte is still one byte.
                            add_frag.push((
                                ssa_key(&f.name, &z.dst),
                                fp(&frag, &param_keys, &f.name, &z.val),
                            ))
                        }
                        Inst::Sext(z) if fv(&frag, &param_keys, &f.name, &z.val) => {
                            add_frag.push((
                                ssa_key(&f.name, &z.dst),
                                fp(&frag, &param_keys, &f.name, &z.val),
                            ))
                        }
                        Inst::Trunc(z) if fv(&frag, &param_keys, &f.name, &z.val) => {
                            add_frag.push((
                                ssa_key(&f.name, &z.dst),
                                fp(&frag, &param_keys, &f.name, &z.val),
                            ))
                        }
                        Inst::Freeze(z) if fv(&frag, &param_keys, &f.name, &z.val) => add_frag
                            .push((
                                ssa_key(&f.name, &z.dst),
                                fp(&frag, &param_keys, &f.name, &z.val),
                            )),
                        Inst::Bin(x) => {
                            let pa = tp(&t, &param_keys, prov, &f.name, &x.a);
                            let pb = tp(&t, &param_keys, prov, &f.name, &x.b);
                            let fa = fp(&frag, &param_keys, &f.name, &x.a);
                            let fb = fp(&frag, &param_keys, &f.name, &x.b);
                            let (ta, tb) = (ts_bool(&pa), ts_bool(&pb));
                            let (fa_b, fb_b) = (ts_bool(&fa), ts_bool(&fb));
                            if bin_taints(x, ta, tb) {
                                let mut p = pa;
                                ts_union(&mut p, &pb);
                                add.push((ssa_key(&f.name, &x.dst), p));
                            } else if matches!(
                                x.op,
                                ir::BinOp::Or | ir::BinOp::Xor | ir::BinOp::Add
                            ) && fa_b
                                && fb_b
                            {
                                // Two fragments joined rebuild the address.
                                let mut p = fa;
                                ts_union(&mut p, &fb);
                                add.push((ssa_key(&f.name, &x.dst), p));
                            } else if matches!(
                                x.op,
                                ir::BinOp::And
                                    | ir::BinOp::Shl
                                    | ir::BinOp::LShr
                                    | ir::BinOp::AShr
                                    | ir::BinOp::Mul
                                    | ir::BinOp::Or
                                    | ir::BinOp::Xor
                                    | ir::BinOp::Add
                            ) && (ta || tb || fa_b || fb_b)
                            {
                                // Taking, scaling, or byte-identity ALU over an
                                // address (or a fragment) stays a fragment; a
                                // two-fragment join taints in the arm above.
                                let mut p = pa;
                                ts_union(&mut p, &pb);
                                ts_union(&mut p, &fa);
                                ts_union(&mut p, &fb);
                                add_frag.push((ssa_key(&f.name, &x.dst), p));
                            }
                        }
                        Inst::Select(s)
                            if !s.ptr
                                && (tv(&t, &param_keys, prov, &f.name, &s.a)
                                    || tv(&t, &param_keys, prov, &f.name, &s.b)) =>
                        {
                            let mut p = tp(&t, &param_keys, prov, &f.name, &s.a);
                            ts_union(&mut p, &tp(&t, &param_keys, prov, &f.name, &s.b));
                            add.push((ssa_key(&f.name, &s.dst), p))
                        }
                        Inst::Select(s)
                            if !s.ptr
                                && (fv(&frag, &param_keys, &f.name, &s.a)
                                    || fv(&frag, &param_keys, &f.name, &s.b)) =>
                        {
                            let mut p = fp(&frag, &param_keys, &f.name, &s.a);
                            ts_union(&mut p, &fp(&frag, &param_keys, &f.name, &s.b));
                            add_frag.push((ssa_key(&f.name, &s.dst), p))
                        }
                        Inst::Phi(p)
                            if !p.ptr
                                && p.incoming
                                    .iter()
                                    .any(|(v, _)| tv(&t, &param_keys, prov, &f.name, v)) =>
                        {
                            let mut ps = TState::default();
                            for (v, _) in p.incoming.iter() {
                                ts_union(&mut ps, &tp(&t, &param_keys, prov, &f.name, v));
                            }
                            add.push((ssa_key(&f.name, &p.dst), ps))
                        }
                        Inst::Phi(p)
                            if !p.ptr
                                && p.incoming
                                    .iter()
                                    .any(|(v, _)| fv(&frag, &param_keys, &f.name, v)) =>
                        {
                            let mut ps = TState::default();
                            for (v, _) in p.incoming.iter() {
                                ts_union(&mut ps, &fp(&frag, &param_keys, &f.name, v));
                            }
                            add_frag.push((ssa_key(&f.name, &p.dst), ps))
                        }
                        Inst::Load(l) if !l.ptr_ty => {
                            if let Some((slot, off, _)) = loc(&f.name, &l.ptr) {
                                if mt.query(&slot, off, u32::from(l.ty.bytes())) {
                                    // Memory persists across calls, so a
                                    // tainted load may reach any caller.
                                    add.push((
                                        ssa_key(&f.name, &l.dst),
                                        TState {
                                            indep: true,
                                            deps: HashSet::new(),
                                        },
                                    ));
                                }
                            }
                        }
                        Inst::Store(s) => {
                            // Existential taint only: a provisional dep is
                            // not enough to taint memory, or clean-param
                            // stores would mark every slot and every later
                            // load would refuse (epic-cc#1324). The loop
                            // re-fires this store when `ptaint` grows.
                            let st = tp(&t, &param_keys, prov, &f.name, &s.val);
                            if st.indep || st.deps.iter().any(|d| ptaint.contains(d)) {
                                if let Some((slot, off, fake)) = loc(&f.name, &s.ptr) {
                                    mt.mark(&slot, off, u32::from(s.ty.bytes()));
                                    if fake {
                                        mt.all = true;
                                    }
                                }
                            }
                        }
                        Inst::Memcpy(mc) => {
                            if let (Some((ss, so, _)), Some((ds, dof, dfake))) =
                                (val_loc(&f.name, &mc.src), val_loc(&f.name, &mc.dst))
                            {
                                let n = match &mc.len {
                                    ir::MemLen::Const(n) => Some(u32::from(*n)),
                                    ir::MemLen::Reg(_) => None,
                                };
                                match n {
                                    Some(n)
                                        if so.1 == 0 && dof.1 == 0 && !mt.wide.contains(&ss) =>
                                    {
                                        for k in 0..n {
                                            if mt.bytes.contains(&(ss.clone(), so.0 + k)) {
                                                mt.mark(&ds, (dof.0 + k, 0), 1);
                                            }
                                        }
                                    }
                                    _ => {
                                        // Unknown length or index: any taint in the
                                        // source slot may be copied anywhere in dst.
                                        if mt.query(&ss, (0, 1), 1) {
                                            mt.mark(&ds, (0, 1), 1);
                                        }
                                    }
                                }
                                if dfake && mt.bytes.iter().any(|(s, _)| s == &ss) {
                                    mt.all = true;
                                }
                            }
                        }
                        Inst::Call(c) => {
                            // Context-sensitive (epic-cc#1324): each result
                            // substitutes the callee params it depends on
                            // with this site's actual argument states, so a
                            // tainted caller no longer taints every
                            // caller's result. Param taints stay merged:
                            // uses inside the callee refuse existentially.
                            let targets: Vec<&String> = if c.callees.is_empty() {
                                vec![&c.func]
                            } else {
                                c.callees.iter().collect()
                            };
                            for name in targets {
                                let Some(callee) = m.funcs.iter().find(|x| &x.name == name) else {
                                    continue;
                                };
                                for (p, a) in callee.params.iter().zip(c.args.iter()) {
                                    // The actual's own taint, or a taint
                                    // through one of the caller's params.
                                    let sa = tp(&t, &param_keys, prov, &f.name, &a.val);
                                    if sa.indep || sa.deps.iter().any(|d| ptaint.contains(d)) {
                                        ptaint.insert(ssa_key(name, &p.name));
                                    }
                                    let fa = fp(&frag, &param_keys, &f.name, &a.val);
                                    if fa.indep || fa.deps.iter().any(|d| pfrag.contains(d)) {
                                        pfrag.insert(ssa_key(name, &p.name));
                                    }
                                }
                                if let Some(d) = &c.dst {
                                    let mut rs = TState::default();
                                    let mut rfs = TState::default();
                                    for bb in &callee.blocks {
                                        for ri in &bb.insts {
                                            if let Inst::Ret(Some((_, v)), _) = ri {
                                                ts_union(
                                                    &mut rs,
                                                    &tp(&t, &param_keys, prov, name, v),
                                                );
                                                ts_union(
                                                    &mut rfs,
                                                    &fp(&frag, &param_keys, name, v),
                                                );
                                            }
                                        }
                                    }
                                    let mut ds = TState {
                                        indep: rs.indep,
                                        deps: HashSet::new(),
                                    };
                                    for q in &rs.deps {
                                        let qi = callee
                                            .params
                                            .iter()
                                            .position(|p| ssa_key(name, &p.name) == *q);
                                        match qi.and_then(|qi| c.args.get(qi)) {
                                            Some(a) => {
                                                let sa = tp(&t, &param_keys, prov, &f.name, &a.val);
                                                ds.indep |= sa.indep;
                                                ds.deps.extend(sa.deps.iter().cloned());
                                            }
                                            // Unknown arity: stay refused.
                                            None => ds.indep = true,
                                        }
                                    }
                                    if ts_bool(&ds) {
                                        add.push((ssa_key(&f.name, d), ds));
                                    }
                                    let mut dfs = TState {
                                        indep: rfs.indep,
                                        deps: HashSet::new(),
                                    };
                                    for q in &rfs.deps {
                                        let qi = callee
                                            .params
                                            .iter()
                                            .position(|p| ssa_key(name, &p.name) == *q);
                                        match qi.and_then(|qi| c.args.get(qi)) {
                                            Some(a) => {
                                                let fa = fp(&frag, &param_keys, &f.name, &a.val);
                                                dfs.indep |= fa.indep;
                                                dfs.deps.extend(fa.deps.iter().cloned());
                                            }
                                            None => dfs.indep = true,
                                        }
                                    }
                                    if ts_bool(&dfs) {
                                        add_frag.push((ssa_key(&f.name, d), dfs));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    for (k, ps) in add {
                        if ts_bool(&ps) {
                            let e = t.entry(k).or_default();
                            ts_union(e, &ps);
                        }
                    }
                    for (k, ps) in add_frag {
                        if ts_bool(&ps) {
                            let e = frag.entry(k).or_default();
                            ts_union(e, &ps);
                        }
                    }
                }
            }
        }
        let after = state_sizes(&t, &frag, &ptaint, &pfrag, &mt);
        if before == after {
            break;
        }
    }
    // Deferred existential evaluation: a value depending on params is
    // dangerous exactly when some call site passed a tainted actual.
    // Deps hold param keys, which `ptaint` keys the same way, so no
    // function lookup is needed.
    prov.int_flash = t
        .iter()
        .filter(|(_, s)| s.indep || s.deps.iter().any(|d| ptaint.contains(d)))
        .map(|(k, _)| k.clone())
        .collect();
}

/// Prove the integer-domain values that certainly carry a flash table
/// address, so an `inttoptr` of one lowers through `TBLPTR` instead of
/// being refused (epic-cc#1346).
///
/// The maybe-taint in `add_flash_laundering` says a value *may* carry a
/// flash address; this says it *does*. The difference is the space
/// proof: a value is definite only when every path that produces it
/// yields a flash address, so the dereference dispatches statically
/// rather than on a runtime tag. A value that may be flash or RAM stays
/// in the refusal.
///
/// The proof is a greatest fixpoint by removal: every value starts
/// assumed definite, and a value is dropped as soon as its derivation
/// fails under the current state. That is the sound direction (a value
/// is claimed only when every path agrees), and the state only shrinks,
/// so the iteration terminates.
///
/// A value is definite when:
///
/// - it is a flash const global, or a pointer derived from one;
/// - it is a two-byte load from a location whose two bytes are
///   definite;
/// - it is a copy (zext, sext, trunc, freeze) of a definite value;
/// - it is an add or a subtract of a definite value and a constant,
///   which keeps the address (a difference of two addresses is a
///   length, and a mask or a shift leaves a fragment, so neither is
///   definite);
/// - it is a select, a phi, or a call result whose every arm is
///   definite.
///
/// Memory bytes are definite when every write to them writes a
/// definite value, so a slot that also takes RAM loses the proof for
/// exactly those bytes. A location a pointer operand names is direct
/// only for a global or an object slot (alloca, byval param); every
/// other pointer names an unknown location, whose load is never
/// definite and whose store voids the whole memory proof.
///
/// `prov.int_flash_ptr` holds the pointer regs that hold a flash
/// address: an `inttoptr` of a definite value, and every pointer
/// derived from it (a GEP over it, a select or a phi arm). A
/// dereference through one seeds `TBLPTR` from the slot bytes.
pub fn add_int_flash_definite(
    prov: &mut FlashProvenance,
    m: &Module,
    resolved: &PtrResolution,
    is_flash_const: impl Fn(&str) -> bool,
) {
    // Object slots: an alloca or a byval param. Their bytes are the
    // object itself, so a load through them reads a known location.
    let mut objects: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Alloca(a) = i {
                    objects.insert(ssa_key(&f.name, &a.dst));
                }
            }
        }
        for p in &f.params {
            if p.byval.is_some() {
                objects.insert(ssa_key(&f.name, &p.name));
            }
        }
    }
    // Functions reachable through an indirect call: their params take
    // an unknown value on that path, so no proof survives it.
    let mut indirect: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(c) = i {
                    indirect.extend(c.callees.iter().cloned());
                }
            }
        }
    }

    // Where a pointer operand points. `Fixed` is a known location at a
    // constant byte offset; every other shape names a location no
    // proof can bound, so a load through it is never definite.
    enum Loc {
        Fixed(String, u32),
        Unknown,
    }
    let loc = |func: &str, ptr: &str| -> Loc {
        if let Some(g) = ptr.strip_prefix('@') {
            return Loc::Fixed(format!("@{g}"), 0);
        }
        let Some(r) = ptr.strip_prefix('%') else {
            return Loc::Unknown;
        };
        match resolved.get(&ssa_key(func, r)) {
            Some((Base::Global(g), k, terms)) => {
                if terms.is_empty() {
                    Loc::Fixed(format!("@{g}"), u32::from(*k))
                } else {
                    Loc::Unknown
                }
            }
            Some((Base::Slot(sname, _), k, terms)) => {
                let key = ssa_key(func, sname);
                if terms.is_empty() && objects.contains(&key) {
                    Loc::Fixed(key, u32::from(*k))
                } else {
                    Loc::Unknown
                }
            }
            None => Loc::Unknown,
        }
    };

    // A write to a location. `Val` writes a value, `Copy` moves bytes
    // from one location to another, and `Init` names the const a
    // global's initializer holds.
    enum Write {
        Val {
            func: String,
            val: ir::Val,
            block: String,
            index: usize,
        },
        Copy {
            func: String,
            sslot: String,
            soff: u32,
            doff: u32,
            block: String,
            index: usize,
        },
        Init {
            target: String,
        },
    }
    let mut writes: Vec<Write> = Vec::new();
    // The writes covering each byte, as indices into `writes`.
    let mut writes_by: HashMap<(String, u32), Vec<usize>> = HashMap::new();
    // The reads of each byte, with the place of the read, so a write is
    // known to have happened by every read of the byte.
    let mut reads_by: HashMap<(String, u32), Vec<(String, String, usize)>> = HashMap::new();
    // A write through a pointer no proof can resolve: every byte of
    // memory may hold it, so no byte is definite.
    let mut void_all = false;

    // The slot a value names, for a memcpy side. A dynamic term makes
    // the location runtime-dependent, so the copy names no known bytes
    // and the caller voids the whole memory proof.
    let val_slot = |func: &str, v: &ir::Val| -> Option<(String, u32)> {
        match v {
            ir::Val::Global(g) => Some((format!("@{g}"), 0)),
            ir::Val::Reg(r) => match resolved.get(&ssa_key(func, r)) {
                Some((Base::Global(g), k, terms)) if terms.is_empty() => {
                    Some((format!("@{g}"), u32::from(*k)))
                }
                Some((Base::Slot(sname, _), k, terms)) if terms.is_empty() => {
                    Some((ssa_key(func, sname), u32::from(*k)))
                }
                _ => None,
            },
            ir::Val::Const(_) => None,
        }
    };

    // How a pointer slot receives its value.
    enum SlotRule {
        /// An `inttoptr` copies a definite integer into the slot.
        IntToPtr { func: String, val: ir::Val },
        /// A pointer join takes the value of every arm.
        Join { func: String, arms: Vec<ir::Val> },
        /// A pointer load copies the source's two bytes.
        LoadPtr { func: String, ptr: String },
        /// A pointer param receives the actual of every call site.
        Param { calls: Vec<(String, ir::Val)> },
        /// A pointer call result holds what the callee returns.
        CallPtr { callee: String },
    }
    let mut slot_rule: HashMap<String, SlotRule> = HashMap::new();
    // The instruction defining each value, keyed `{func}::{name}`.
    let mut defs: HashMap<String, (String, &Inst)> = HashMap::new();

    for f in &m.funcs {
        for b in &f.blocks {
            for (inst_index, i) in b.insts.iter().enumerate() {
                let func = f.name.clone();
                match i {
                    Inst::Store(s) => match loc(&func, &s.ptr) {
                        Loc::Fixed(slot, off) => {
                            let idx = writes.len();
                            writes.push(Write::Val {
                                func: func.clone(),
                                val: s.val.clone(),
                                block: b.label.clone(),
                                index: inst_index,
                            });
                            for k in 0..u32::from(s.ty.bytes()) {
                                writes_by
                                    .entry((slot.clone(), off + k))
                                    .or_default()
                                    .push(idx);
                            }
                        }
                        Loc::Unknown => void_all = true,
                    },
                    Inst::Memcpy(mc) => {
                        let src = val_slot(&func, &mc.src);
                        let dst = val_slot(&func, &mc.dst);
                        let (Some((ss, so)), Some((ds, doff))) = (src, dst) else {
                            void_all = true;
                            continue;
                        };
                        let ir::MemLen::Const(n) = mc.len else {
                            void_all = true;
                            continue;
                        };
                        let idx = writes.len();
                        writes.push(Write::Copy {
                            func: func.clone(),
                            sslot: ss,
                            soff: so,
                            doff,
                            block: b.label.clone(),
                            index: inst_index,
                        });
                        for k in 0..u32::from(n) {
                            writes_by
                                .entry((ds.clone(), doff + k))
                                .or_default()
                                .push(idx);
                        }
                    }
                    Inst::Asm(a) => {
                        // An asm output writes bytes no proof can name.
                        if a.clobbers_memory {
                            void_all = true;
                        }
                        for op in &a.operands {
                            if op.constraint.starts_with('=') || op.constraint.starts_with('+') {
                                match loc(&func, &op.ptr) {
                                    Loc::Fixed(slot, off) => {
                                        let idx = writes.len();
                                        writes.push(Write::Val {
                                            func: func.clone(),
                                            val: ir::Val::Const(-1),
                                            block: b.label.clone(),
                                            index: inst_index,
                                        });
                                        writes_by.entry((slot, off)).or_default().push(idx);
                                    }
                                    Loc::Unknown => void_all = true,
                                }
                            }
                        }
                    }
                    Inst::Load(l) if !l.ptr_ty => {
                        // A read observes the slot's bytes at this
                        // place, so a write must have happened by it.
                        if let Loc::Fixed(slot, off) = loc(&func, &l.ptr) {
                            for k in 0..u32::from(l.ty.bytes()) {
                                reads_by.entry((slot.clone(), off + k)).or_default().push((
                                    func.clone(),
                                    b.label.clone(),
                                    inst_index,
                                ));
                            }
                        }
                    }
                    Inst::IntToPtr(p) => {
                        slot_rule.insert(
                            ssa_key(&func, &p.dst),
                            SlotRule::IntToPtr {
                                func: func.clone(),
                                val: p.val.clone(),
                            },
                        );
                    }
                    Inst::Select(s) if s.ptr => {
                        slot_rule.insert(
                            ssa_key(&func, &s.dst),
                            SlotRule::Join {
                                func: func.clone(),
                                arms: vec![s.a.clone(), s.b.clone()],
                            },
                        );
                    }
                    Inst::Phi(p) if p.ptr => {
                        slot_rule.insert(
                            ssa_key(&func, &p.dst),
                            SlotRule::Join {
                                func: func.clone(),
                                arms: p.incoming.iter().map(|(v, _)| v.clone()).collect(),
                            },
                        );
                    }
                    Inst::Load(l) if l.ptr_ty => {
                        slot_rule.insert(
                            ssa_key(&func, &l.dst),
                            SlotRule::LoadPtr {
                                func: func.clone(),
                                ptr: l.ptr.clone(),
                            },
                        );
                    }
                    Inst::Call(c) => {
                        // A call result used in pointer position is a
                        // pointer value: its slot holds the returned
                        // address.
                        if let Some(d) = &c.dst {
                            let key = ssa_key(&func, d);
                            if resolved.contains_key(&key) {
                                slot_rule.insert(
                                    key,
                                    SlotRule::CallPtr {
                                        callee: c.func.clone(),
                                    },
                                );
                            }
                        }
                    }
                    _ => {}
                }
                // Record the defining instruction of every value.
                let dst: Option<&String> = match i {
                    Inst::Zext(z) => Some(&z.dst),
                    Inst::Sext(z) => Some(&z.dst),
                    Inst::Trunc(z) => Some(&z.dst),
                    Inst::Freeze(z) => Some(&z.dst),
                    Inst::Bin(x) => Some(&x.dst),
                    Inst::Select(s) => Some(&s.dst),
                    Inst::Phi(p) => Some(&p.dst),
                    Inst::Call(c) => c.dst.as_ref(),
                    Inst::Load(l) => Some(&l.dst),
                    Inst::IntToPtr(p) => Some(&p.dst),
                    Inst::Gep(g) => Some(&g.dst),
                    _ => None,
                };
                if let Some(d) = dst {
                    defs.insert(ssa_key(&func, d), (func.clone(), i));
                }
            }
        }
    }
    // A global's initializer names the consts it holds: the two bytes
    // of a ref hold that const's address.
    for g in &m.globals {
        let key = format!("@{}", g.name);
        for (pos, target, _) in &g.refs {
            let idx = writes.len();
            writes.push(Write::Init {
                target: target.clone(),
            });
            for k in 0..2u32 {
                writes_by
                    .entry((key.clone(), *pos as u32 + k))
                    .or_default()
                    .push(idx);
            }
        }
    }
    // A pointer param receives the actual of every call site.
    for f in &m.funcs {
        for (idx, p) in f.params.iter().enumerate() {
            if !p.ptr || p.byval.is_some() || p.sret {
                continue;
            }
            let mut calls: Vec<(String, ir::Val)> = Vec::new();
            if !indirect.contains(&f.name) {
                for g in &m.funcs {
                    for b in &g.blocks {
                        for i in &b.insts {
                            if let Inst::Call(c) = i {
                                if c.func == f.name {
                                    if let Some(arg) = c.args.get(idx) {
                                        calls.push((g.name.clone(), arg.val.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            slot_rule.insert(ssa_key(&f.name, &p.name), SlotRule::Param { calls });
        }
    }

    // The state: the values and the memory bytes proven definite.
    let mut flash_val: HashSet<String> = defs.keys().cloned().collect();
    let mut flash_bytes: HashSet<(String, u32)> = if void_all {
        HashSet::new()
    } else {
        writes_by.keys().cloned().collect()
    };
    if !void_all {
        for key in slot_rule.keys() {
            for k in 0..2u32 {
                flash_bytes.insert((key.clone(), k));
            }
        }
    }

    // Whether a value is definitely a flash address.
    let is_def = |flash_val: &HashSet<String>, func: &str, v: &ir::Val| -> bool {
        match v {
            ir::Val::Global(n) => is_flash_const(n),
            ir::Val::Reg(r) => flash_val.contains(&ssa_key(func, r)),
            ir::Val::Const(_) => false,
        }
    };
    // Whether the two bytes a location holds are definite.
    let two_def = |flash_bytes: &HashSet<(String, u32)>, slot: &str, off: u32| -> bool {
        flash_bytes.contains(&(slot.to_string(), off))
            && flash_bytes.contains(&(slot.to_string(), off + 1))
    };

    // Whether a call's returned value is definite at this site, with the
    // callee's integer params substituted by the actuals.
    let call_returns_def = |flash_val: &HashSet<String>, func: &str, c: &ir::Call| -> bool {
        if !c.callees.is_empty() {
            // An indirect call returns an unknown value.
            return false;
        }
        let Some(callee) = m.funcs.iter().find(|x| x.name == c.func) else {
            return false;
        };
        let mut returns: Vec<&ir::Val> = Vec::new();
        for bb in &callee.blocks {
            for ri in &bb.insts {
                if let Inst::Ret(Some((_, v)), _) = ri {
                    returns.push(v);
                }
            }
        }
        // The names the callee defines, so a returned register that the
        // body overwrites is not the parameter's incoming value. The IR
        // is a slot IR, so a name may be reused.
        let mut defined: HashSet<String> = HashSet::new();
        for bb in &callee.blocks {
            for ri in &bb.insts {
                let dst: Option<&String> = match ri {
                    Inst::Load(l) => Some(&l.dst),
                    Inst::Bin(x) => Some(&x.dst),
                    Inst::Zext(z) => Some(&z.dst),
                    Inst::Sext(s) => Some(&s.dst),
                    Inst::Trunc(t) => Some(&t.dst),
                    Inst::Freeze(z) => Some(&z.dst),
                    Inst::IntToPtr(p) => Some(&p.dst),
                    Inst::Icmp(i) => Some(&i.dst),
                    Inst::Select(s) => Some(&s.dst),
                    Inst::Phi(p) => Some(&p.dst),
                    Inst::Call(cc) => cc.dst.as_ref(),
                    Inst::Gep(g) => Some(&g.dst),
                    Inst::Alloca(a) => Some(&a.dst),
                    Inst::VaArg(v) => Some(&v.dst),
                    _ => None,
                };
                if let Some(d) = dst {
                    defined.insert(d.clone());
                }
            }
        }
        !returns.is_empty()
            && returns.iter().all(|v| match v {
                ir::Val::Global(n) => is_flash_const(n),
                ir::Val::Reg(r) => {
                    let Some(idx) = callee.params.iter().position(|p| &p.name == r) else {
                        return false;
                    };
                    // Only an integer param carries an address through
                    // the call: a pointer param has its own slot rules.
                    if callee.params[idx].ptr {
                        return false;
                    }
                    // The body must not have overwritten the name: the
                    // returned value is the parameter's incoming value
                    // only when nothing else wrote it.
                    if defined.contains(r) {
                        return false;
                    }
                    let Some(arg) = c.args.get(idx) else {
                        return false;
                    };
                    is_def(flash_val, func, &arg.val)
                }
                ir::Val::Const(_) => false,
            })
    };

    // The derivation of one value, under the current state.
    let derive =
        |flash_val: &HashSet<String>, flash_bytes: &HashSet<(String, u32)>, key: &str| -> bool {
            let Some((func, inst)) = defs.get(key) else {
                return false;
            };
            match inst {
                // A load reads the bytes the location holds: the value is a
                // flash address when both address bytes are definite. A
                // narrower or wider load reads no address.
                Inst::Load(l) => {
                    if l.ty.bytes() != 2 {
                        return false;
                    }
                    match loc(func, &l.ptr) {
                        Loc::Fixed(slot, off) => two_def(flash_bytes, &slot, off),
                        Loc::Unknown => false,
                    }
                }
                // A copy of a definite value keeps the address.
                Inst::Zext(z) => z.from.bytes() == 2 && is_def(flash_val, func, &z.val),
                Inst::Sext(z) => z.from.bytes() == 2 && is_def(flash_val, func, &z.val),
                Inst::Trunc(z) => z.to.bytes() == 2 && is_def(flash_val, func, &z.val),
                Inst::Freeze(z) => is_def(flash_val, func, &z.val),
                // A constant offset keeps the address; a difference of two
                // addresses is a length, and a mask or a shift leaves a
                // fragment.
                Inst::Bin(x) => {
                    let a_def = is_def(flash_val, func, &x.a);
                    let b_def = is_def(flash_val, func, &x.b);
                    let a_const = matches!(x.a, ir::Val::Const(_));
                    let b_const = matches!(x.b, ir::Val::Const(_));
                    match x.op {
                        ir::BinOp::Add => (a_def && b_const) || (b_def && a_const),
                        ir::BinOp::Sub => a_def && b_const,
                        _ => false,
                    }
                }
                // A join is definite when every arm is.
                Inst::Select(s) => is_def(flash_val, func, &s.a) && is_def(flash_val, func, &s.b),
                Inst::Phi(p) => {
                    !p.incoming.is_empty()
                        && p.incoming.iter().all(|(v, _)| is_def(flash_val, func, v))
                }
                // A call result is definite when every returned value is,
                // with the callee's integer params substituted by the
                // actuals of this site.
                Inst::Call(c) => call_returns_def(flash_val, func, c),
                // A pointer value: definite when the integer it copies is.
                // A GEP is not one: `resolve_pointers` folds it away, so its
                // own slot never holds the address (the address materializes
                // at each use), and a read of that slot is not an address.
                Inst::IntToPtr(p) => is_def(flash_val, func, &p.val),
                _ => false,
            }
        };

    // The blocks each block dominates, so a write in one block is known
    // to have happened when a later block reads.
    let dominators: HashMap<String, HashMap<String, HashSet<String>>> = m
        .funcs
        .iter()
        .map(|f| (f.name.clone(), func_dominators(f)))
        .collect();
    // Whether a write has happened by a read: the same block counts
    // when the write comes first, and another block counts when it
    // dominates the read's block.
    let write_precedes = |write_func: &str,
                          write_block: &str,
                          write_index: usize,
                          read_func: &str,
                          read_block: &str,
                          read_index: usize|
     -> bool {
        if write_func != read_func {
            // A write in another function happens at a call site the
            // proof does not track, so it never precedes a read here.
            return false;
        }
        if write_block == read_block {
            write_index < read_index
        } else {
            dominators
                .get(read_func)
                .and_then(|d| d.get(read_block))
                .is_some_and(|set| set.contains(write_block))
        }
    };

    // The derivation of one memory byte, under the current state.
    let derive_byte = |flash_val: &HashSet<String>,
                       flash_bytes: &HashSet<(String, u32)>,
                       slot: &str,
                       off: u32|
     -> bool {
        // A slot whose value comes from a rule (an `inttoptr`, a join, a
        // pointer load, a param, a call result) is definite when the
        // rule holds; the memory writes do not apply to it.
        if let Some(rule) = slot_rule.get(slot) {
            return match rule {
                SlotRule::IntToPtr { func, val } => is_def(flash_val, func, val),
                SlotRule::Join { func, arms } => {
                    !arms.is_empty() && arms.iter().all(|v| is_def(flash_val, func, v))
                }
                SlotRule::LoadPtr { func, ptr } => match loc(func, ptr) {
                    Loc::Fixed(s, o) => two_def(flash_bytes, &s, o),
                    Loc::Unknown => false,
                },
                SlotRule::Param { calls } => {
                    !calls.is_empty() && calls.iter().all(|(f, v)| is_def(flash_val, f, v))
                }
                SlotRule::CallPtr { callee } => {
                    // The slot holds what the callee returns. Every
                    // call site of that callee must agree, and each
                    // site's own arguments decide the callee's
                    // integer params.
                    let mut sites: Vec<&ir::Call> = Vec::new();
                    for g in &m.funcs {
                        for b in &g.blocks {
                            for i in &b.insts {
                                if let Inst::Call(cc) = i {
                                    if cc.func == *callee {
                                        sites.push(cc);
                                    }
                                }
                            }
                        }
                    }
                    !sites.is_empty()
                        && sites
                            .iter()
                            .all(|cc| call_returns_def(flash_val, &cc.func, cc))
                }
            };
        }
        // Every write to the byte must write a definite value, and
        // every read of the byte must observe one of those writes: a
        // read before a later write sees the earlier value, so the
        // byte is not definite.
        let Some(list) = writes_by.get(&(slot.to_string(), off)) else {
            return false;
        };
        if list.is_empty() {
            return false;
        }
        let reads = reads_by.get(&(slot.to_string(), off));
        list.iter().all(|idx| match &writes[*idx] {
            Write::Val {
                func,
                val,
                block,
                index,
            } => {
                if !is_def(flash_val, func, val) {
                    return false;
                }
                // The write must have happened by every read of the
                // byte.
                match reads {
                    Some(rs) => rs
                        .iter()
                        .all(|(rf, rb, ri)| write_precedes(func, block, *index, rf, rb, *ri)),
                    None => true,
                }
            }
            Write::Copy {
                func,
                sslot,
                soff,
                doff,
                block,
                index,
            } => {
                if !flash_bytes.contains(&(sslot.clone(), soff + (off - doff))) {
                    return false;
                }
                match reads {
                    Some(rs) => rs
                        .iter()
                        .all(|(rf, rb, ri)| write_precedes(func, block, *index, rf, rb, *ri)),
                    None => true,
                }
            }
            Write::Init { target } => is_flash_const(target),
        })
    };

    // The greatest fixpoint by removal: a value or byte that no longer
    // derives is dropped, and the state only shrinks.
    loop {
        let mut changed = false;
        for key in flash_val.clone() {
            if !derive(&flash_val, &flash_bytes, &key) {
                flash_val.remove(&key);
                changed = true;
            }
        }
        for (slot, off) in flash_bytes.clone() {
            if !derive_byte(&flash_val, &flash_bytes, &slot, off) {
                flash_bytes.remove(&(slot, off));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // The pointer regs that hold a flash address: the `inttoptr` dsts
    // whose source integer is definite, and the pointers derived from
    // them. The walk starts at the `inttoptr` dsts alone: a slot that
    // merely holds a definite integer is data, not an address.
    //
    // Within the candidates, the set is a fixpoint by removal, like the
    // value proof. Removal keeps the derivations consistent, so a
    // pointer derived from a dropped one is dropped too.
    let mut ptr: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::IntToPtr(p) = i {
                    let key = ssa_key(&f.name, &p.dst);
                    if flash_val.contains(&key) {
                        ptr.insert(key);
                    }
                }
            }
        }
    }
    // The candidates: the pointers derived from an `inttoptr` dst (a
    // GEP base, a select arm, a phi arm, a freeze operand).
    let mut cands: HashSet<String> = ptr.clone();
    let mut grew = true;
    while grew {
        grew = false;
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    let dst: Option<&String> = match i {
                        Inst::Gep(g) => Some(&g.dst),
                        Inst::Select(s) if s.ptr => Some(&s.dst),
                        Inst::Phi(p) if p.ptr => Some(&p.dst),
                        Inst::Freeze(fr) => Some(&fr.dst),
                        _ => None,
                    };
                    let Some(dst) = dst else { continue };
                    let srcs: Vec<String> = match i {
                        Inst::Gep(g) => match &g.base {
                            GepBase::Reg(r) => vec![r.clone()],
                            GepBase::Global(_) => continue,
                        },
                        Inst::Select(s) if s.ptr => [&s.a, &s.b]
                            .iter()
                            .filter_map(|v| match v {
                                ir::Val::Reg(r) => Some(r.clone()),
                                _ => None,
                            })
                            .collect(),
                        Inst::Phi(p) if p.ptr => p
                            .incoming
                            .iter()
                            .filter_map(|(v, _)| match v {
                                ir::Val::Reg(r) => Some(r.clone()),
                                _ => None,
                            })
                            .collect(),
                        Inst::Freeze(fr) => match &fr.val {
                            ir::Val::Reg(r) => vec![r.clone()],
                            _ => continue,
                        },
                        _ => continue,
                    };
                    if srcs.is_empty() {
                        continue;
                    }
                    let key = ssa_key(&f.name, dst);
                    if srcs.iter().any(|r| cands.contains(&ssa_key(&f.name, r)))
                        && cands.insert(key)
                    {
                        grew = true;
                    }
                }
            }
        }
    }
    ptr = cands.clone();
    // The derivation of one pointer, under the current state: a GEP
    // over a flash pointer, a join whose every arm is a flash pointer
    // (a flash const global arm counts), and a freeze of one.
    //
    // A join that mixes a flash pointer with a non-flash value does not
    // derive, so its result is withdrawn.
    let derives = |ptr: &HashSet<String>, func: &str, inst: &Inst| -> bool {
        match inst {
            Inst::IntToPtr(p) => flash_val.contains(&ssa_key(func, &p.dst)),
            Inst::Gep(g) => match &g.base {
                GepBase::Reg(r) => ptr.contains(&ssa_key(func, r)),
                GepBase::Global(n) => is_flash_const(n),
            },
            Inst::Select(s) if s.ptr => [&s.a, &s.b].iter().all(|v| match v {
                ir::Val::Reg(r) => ptr.contains(&ssa_key(func, r)),
                ir::Val::Global(n) => is_flash_const(n),
                ir::Val::Const(_) => false,
            }),
            Inst::Phi(p) if p.ptr => {
                !p.incoming.is_empty()
                    && p.incoming.iter().all(|(v, _)| match v {
                        ir::Val::Reg(r) => ptr.contains(&ssa_key(func, r)),
                        ir::Val::Global(n) => is_flash_const(n),
                        ir::Val::Const(_) => false,
                    })
            }
            Inst::Freeze(fr) => match &fr.val {
                ir::Val::Reg(r) => ptr.contains(&ssa_key(func, r)),
                ir::Val::Global(n) => is_flash_const(n),
                ir::Val::Const(_) => false,
            },
            _ => false,
        }
    };
    // The greatest fixpoint by removal: a pointer that no longer
    // derives is dropped, and the state only shrinks.
    loop {
        let mut changed = false;
        for f in &m.funcs {
            for b in &f.blocks {
                for i in &b.insts {
                    let dst: Option<&String> = match i {
                        Inst::IntToPtr(p) => Some(&p.dst),
                        Inst::Gep(g) => Some(&g.dst),
                        Inst::Select(s) if s.ptr => Some(&s.dst),
                        Inst::Phi(p) if p.ptr => Some(&p.dst),
                        Inst::Freeze(fr) => Some(&fr.dst),
                        _ => None,
                    };
                    let Some(dst) = dst else { continue };
                    let key = ssa_key(&f.name, dst);
                    if !ptr.contains(&key) {
                        continue;
                    }
                    if derives(&ptr, &f.name, i) {
                        continue;
                    }
                    ptr.remove(&key);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // The maybe-flash joins, classified over the final set: a join
    // whose arms mix a flash pointer with a value that is not one
    // carries flash on one path and RAM on another, so its result is
    // marked mixed and the backend's maybe-flash refusal fires on a
    // dereference through it. The classification runs after the
    // removal fixpoint, so it does not depend on the order the joins
    // were examined in. Only the joins over a candidate are classified.
    let mut mixed_joins: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                let (dst, arms): (&String, Vec<&ir::Val>) = match i {
                    Inst::Select(s) if s.ptr => (&s.dst, vec![&s.a, &s.b]),
                    Inst::Phi(p) if p.ptr => (&p.dst, p.incoming.iter().map(|(v, _)| v).collect()),
                    _ => continue,
                };
                if arms.is_empty() {
                    continue;
                }
                let key = ssa_key(&f.name, dst);
                if !cands.contains(&key) {
                    continue;
                }
                // The flash arms of the join, read off the final set.
                let flash_arms = arms.iter().any(|v| match v {
                    ir::Val::Reg(r) => ptr.contains(&ssa_key(&f.name, r)),
                    ir::Val::Global(n) => is_flash_const(n),
                    ir::Val::Const(_) => false,
                });
                if !flash_arms {
                    continue;
                }
                // Every arm must be a flash arm for the join to be
                // flash; one that is not makes it maybe-flash.
                let all_flash = arms.iter().all(|v| match v {
                    ir::Val::Reg(r) => ptr.contains(&ssa_key(&f.name, r)),
                    ir::Val::Global(n) => is_flash_const(n),
                    ir::Val::Const(_) => false,
                });
                if !all_flash {
                    mixed_joins.insert(key);
                }
            }
        }
    }
    // The maybe-flash joins are mixed pointers: the backend refuses a
    // dereference through one, because no tag survives the join.
    for key in &mixed_joins {
        prov.mixed.insert(key.clone());
    }
    prov.int_flash_ptr = ptr;
}

/// Global names an address value may carry, following pointer
/// derivations (GEP bases, select/phi arms, freeze copies) and folded
/// global bases transitively. Loads, params, and call results stop the
/// walk: their consts are governed where they are written, not where
/// they are read. `keep` selects which globals to report.
fn val_fan_in(
    m: &Module,
    resolved: &PtrResolution,
    func: &str,
    v: &ir::Val,
    keep: impl Fn(&str) -> bool,
) -> HashSet<String> {
    // Pointer derivations per reg: GEP bases, select/phi arms, and
    // freeze copies (a freeze is a byte-identity move of the address).
    let mut arms: HashMap<String, Vec<ir::Val>> = HashMap::new();
    if let Some(f) = m.funcs.iter().find(|x| x.name == func) {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Gep(g) => {
                        arms.insert(
                            g.dst.clone(),
                            vec![match &g.base {
                                GepBase::Global(n) => ir::Val::Global(n.clone()),
                                GepBase::Reg(r) => ir::Val::Reg(r.clone()),
                            }],
                        );
                    }
                    Inst::Select(s) if s.ptr => {
                        arms.insert(s.dst.clone(), vec![s.a.clone(), s.b.clone()]);
                    }
                    Inst::Phi(p) if p.ptr => {
                        arms.insert(
                            p.dst.clone(),
                            p.incoming.iter().map(|(av, _)| av.clone()).collect(),
                        );
                    }
                    Inst::Freeze(fr) => {
                        arms.insert(fr.dst.clone(), vec![fr.val.clone()]);
                    }
                    _ => {}
                }
            }
        }
    }
    let mut out: HashSet<String> = HashSet::new();
    let mut stack = vec![v.clone()];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(top) = stack.pop() {
        match top {
            ir::Val::Global(n) => {
                if keep(&n) {
                    out.insert(n);
                }
            }
            ir::Val::Reg(r) => {
                if !seen.insert(ssa_key(func, &r)) {
                    continue;
                }
                if let Some(conts) = arms.get(&r) {
                    stack.extend(conts.iter().cloned());
                } else if let Some((Base::Global(cg), _, _)) = resolved.get(&ssa_key(func, &r)) {
                    if keep(cg) {
                        out.insert(cg.clone());
                    }
                }
            }
            ir::Val::Const(_) => {}
        }
    }
    out
}

/// Const globals whose addresses the value `v` in `func` may carry
/// (epic-cc#1119).
pub fn const_fan_in(
    m: &Module,
    resolved: &PtrResolution,
    func: &str,
    v: &ir::Val,
) -> HashSet<String> {
    val_fan_in(m, resolved, func, v, |n| {
        m.globals.iter().any(|g| g.name == n && g.is_const)
    })
}

/// Every global name the value `v` in `func` may carry, const or not:
/// the address-taken set behind unknown-store demotion (epic-cc#1119).
pub fn global_fan_in(
    m: &Module,
    resolved: &PtrResolution,
    func: &str,
    v: &ir::Val,
) -> HashSet<String> {
    val_fan_in(m, resolved, func, v, |_| true)
}

#[cfg(test)]
mod tests {
    use super::{is_directive, resolve_pointers, Base};

    /// Two pointer phis that read each other, one through a gep
    /// (epic-cc#1127): the optimistic round seeds both, and the gep over
    /// the first resolves against its seed.
    #[test]
    fn swapped_pointer_phis_resolve_through_the_gep() {
        let m = ir::parse(
            "const .str i8\n\
             const .str.1 i8\n\
             fn main(i16) ()\n\
             block 0:\n\
             br 1\n\
             block 1:\n\
             %3 = phi ptr %4 1 @.str 0\n\
             %4 = phi ptr %6 1 @.str.1 0\n\
             %6 = gep %3 +1\n\
             br 1\n",
        );
        let r = resolve_pointers(&m);
        assert!(
            matches!(r.get("main::3"), Some((Base::Slot(s, true), 0, t)) if s == "3" && t.is_empty())
        );
        assert!(
            matches!(r.get("main::4"), Some((Base::Slot(s, true), 0, t)) if s == "4" && t.is_empty())
        );
        assert!(
            matches!(r.get("main::6"), Some((Base::Slot(s, true), 1, t)) if s == "3" && t.is_empty())
        );
    }

    /// Optimism never masks a broken chain: a phi arm that is neither an
    /// address value nor a pending phi, gep, or select still refuses loud.
    #[test]
    #[should_panic(expected = "cyclic or unresolvable pointer chain")]
    fn phi_with_broken_arm_still_refuses() {
        let m = ir::parse(
            "const .str i8\n\
             const .str.1 i8\n\
             fn main(i16) ()\n\
             block 0:\n\
             br 1\n\
             block 1:\n\
             %3 = phi ptr %9 1 @.str 0\n\
             %4 = phi ptr %6 1 @.str.1 0\n\
             %6 = gep %3 +1\n\
             %9 = add i8 1 2\n\
             br 1\n",
        );
        let _ = resolve_pointers(&m);
    }

    #[test]
    fn bare_directive_matches_name_alone_or_followed_by_whitespace() {
        assert!(is_directive("list", "list"));
        assert!(is_directive("list p=16f877a", "list"));
        assert!(is_directive("list\tp=16f877a", "list"));
        assert!(is_directive("radix dec", "radix"));
        assert!(is_directive("end", "end"));
        assert!(is_directive("end reset", "end"));
    }

    #[test]
    fn label_sharing_a_directive_prefix_is_not_a_directive() {
        assert!(!is_directive("endpoint:", "end"));
        assert!(!is_directive("endpoint: MOVLW 1", "end"));
        assert!(!is_directive("listen: MOVLW 1", "list"));
        assert!(!is_directive("listing", "list"));
        assert!(!is_directive("radix_mode:", "radix"));
        assert!(!is_directive("ending", "end"));
    }
}
