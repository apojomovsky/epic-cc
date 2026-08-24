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

/// Where a local's bytes live. v1 only ever constructs `Direct` — introduced
/// now (docs/29-pic18-port-design.md §2 D-2) so a later frame-pointer phase
/// (recursion/reentrancy) never has to touch the call sites that resolve a
/// local's address, only add a real `Frame` case here and wherever
/// `Slot` values get constructed.
pub enum Slot {
    /// Statically allocated: a direct file address.
    Direct(u16),
    /// Frame-relative, FSR2 + offset. Reserved for the later reentrancy
    /// phase; nothing constructs this yet.
    #[allow(dead_code)]
    Frame(i8),
}

impl Slot {
    /// v1 only ever constructs `Direct`.
    pub fn direct(&self) -> u16 {
        match self {
            Slot::Direct(a) => *a,
            Slot::Frame(_) => {
                unimplemented!("frame-relative slots arrive with the reentrancy phase")
            }
        }
    }
}

/// Parse an alloc-produced address-map text into `HashMap<String, u16>`:
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become map
/// entries (locals keyed `{func}::{name}`); `const <name>` lines list flash
/// globals, which have no RAM address, so they are accepted and skipped —
/// isel reads their bytes from the `Module`, never from a RAM slot.
///
/// Shared between both backends (moved here from `isel`, which only ever
/// had this because `isel-pic18` pulled it in via a hard `isel` dependency
/// that existed for no other reason — see the plan's final-review fix
/// notes). Nothing about this parser is PIC14-specific; it is a plain
/// text-format parser over `alloc`'s output.
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

/// Fold every `Inst::Gep` and pointer-typed `Inst::Select` in `m` to
/// `(base, k, terms)`: `base` is where the chain ultimately starts, `k` is
/// the constant byte offset, `terms` is `Vec<(scale, reg)>` for every
/// dynamic (register-indexed) offset in the chain, inner-to-outer. Keyed
/// `{func}::{reg}` via `ssa_key`, matching every other per-function map in
/// this pipeline.
///
/// Seeds first (byval/sret params, allocas, each its own `Base::Slot`
/// with no offset), then a fixpoint scan over every `Gep` and pointer
/// select: a `GepBase::Reg` folds in its own already-resolved entry (`k`
/// adds, `terms` concatenate inner-first) until the chain bottoms out at a
/// `Global` or a seed. A pointer select folds as `select c, a, b` where
/// `c ? a : b` with `a = base+ka` and `b = base+kb` becomes
/// `base + kb + (ka - kb)*c` (wrapping `u8` difference, so `c=0` picks `b`
/// and `c=1` picks `a`). When `ka > kb` the scale is small (`ka-kb`); when
/// `ka < kb` it wraps to `256-(kb-ka)` and the backend emits the complement
/// efficiently. `legalize` normalizes the common `true->hi` shape to the
/// small scale, but `iselcore` is correct for either order without
/// normalization. A `Gep` whose base is neither a seed nor another
/// (eventually resolvable) `Gep`/select is a bug in an earlier stage and
/// panics loudly, as does a select whose arms do not fold to a common base;
/// a scan that makes no progress with unresolved entries left is a cycle and
/// panics loudly.
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
            // it always fits. Arms that do not fold (different bases, term
            // mismatches, non-reg cond) stay pending and panic below.
            let mut rest_selects = Vec::new();
            for (key, s) in pending_selects {
                if let Some(folded) = fold_select(&s, &resolved, &fname) {
                    assert!(
                        !resolved.contains_key(&key),
                        "iselcore: duplicate definition of pointer reg {key}"
                    );
                    resolved.insert(key, folded);
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

/// Fold a pointer-typed select `select i1 c, ptr a, ptr b` (`c ? a : b`)
/// where `a = base+ka` and `b = base+kb` with identical term sets. The
/// result is `base + kb + (ka - kb)*c` (wrapping `u8`), so `c=0` yields `b`
/// and `c=1` yields `a`. When `ka > kb` the scale is `ka-kb` (small,
/// canonical `true->hi` shape); when `ka < kb` it wraps to `256-(kb-ka)` and
/// isel emits the complement as `lo + (kb-ka)*(1-c)` to keep code size
/// small. Returns `None` when the arms do not fold to a common base, the
/// term sets differ, or the cond is not a reg.
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
    let c = match &s.cond {
        ir::Val::Reg(c) => c.clone(),
        _ => return None,
    };
    let ka = va.1;
    let kb = vb.1;
    if ka == kb {
        // Both arms are the same pointer: the select is a no-op.
        return Some((va.0.clone(), ka, va.2.clone()));
    }
    let d = ka.wrapping_sub(kb);
    let mut terms = vb.2.clone();
    terms.push((d, c));
    Some((vb.0.clone(), kb, terms))
}
