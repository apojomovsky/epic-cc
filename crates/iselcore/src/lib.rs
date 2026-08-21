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
#[derive(Clone, Debug)]
pub enum Base {
    Global(String),
    Slot(String, bool),
}

/// Fold every `Inst::Gep` in `m` to `(base, k, terms)`: `base` is where the
/// chain ultimately starts, `k` is the constant byte offset, `terms` is
/// `Vec<(scale, reg)>` for every dynamic (register-indexed) offset in the
/// chain, inner-to-outer. Keyed `{func}::{reg}` via `ssa_key`, matching
/// every other per-function map in this pipeline.
///
/// Seeds first (byval/sret params, allocas, each its own `Base::Slot`
/// with no offset), then a fixpoint scan over every `Gep`: a `GepBase::Reg`
/// folds in its own already-resolved entry (`k` adds, `terms` concatenate
/// inner-first) until the chain bottoms out at a `Global` or a seed. A
/// `Gep` whose base is neither a seed nor another (eventually resolvable)
/// `Gep` is a bug in an earlier stage and panics loudly; a scan that makes
/// no progress with unresolved geps left is a cycle and panics loudly.
pub fn resolve_pointers(m: &Module) -> HashMap<String, (Base, u8, Vec<(u8, String)>)> {
    let mut geps: HashMap<String, ir::Gep> = HashMap::new();
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
                        } else if geps.contains_key(&rkey) {
                            rest.push((key, g));
                        } else {
                            panic!("iselcore: no gep for pointer %{r} (chain base missing, key {rkey})");
                        }
                    }
                }
            }
            pending = rest;
            if !progressed && !pending.is_empty() {
                let names: Vec<&str> = pending.iter().map(|(k, _)| k.as_str()).collect();
                panic!("iselcore: cyclic gep chain involving {names:?}");
            }
        }
    }
    resolved
}
