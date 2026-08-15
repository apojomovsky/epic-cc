//! Overlay address allocation for the PIC8 pipeline.
//!
//! Globals get sequential, even-aligned (i16) bank-0 addresses starting at
//! `GLOBAL_START`. Every local of every function lives in a frame assigned
//! from the call graph: `base(f) = max over callers of (base(caller) +
//! locals_size(caller))`, roots start at `bank0_start`, so sibling functions
//! (never co-live) share RAM. Frames are checked to fit in bank 0.

use std::collections::{HashMap, HashSet};

use ir::{Inst, Module};

pub const GLOBAL_START: u8 = 0x20;
/// Bank-0 GPRs run 0x00..0x6F; common RAM starts at 0x70. Frames must end
/// before 0x70 (`base + locals_size <= 0x70`).
const BANK0_END: u16 = 0x70;

/// Complete address map: globals keyed by name, locals keyed `{func}::{name}`,
/// plus the total bank-0 span the overlay needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AllocLayout {
    pub globals: HashMap<String, u8>,
    pub locals: HashMap<String, u8>,
    pub total_bank0: u16,
}

/// Assign every address: globals as milestone-2 (sequential, even-aligned i16),
/// locals per the overlay algorithm over the call graph parsed from
/// `edges_text` (`edge <caller> <callee>` lines, order-agnostic; `depth` lines
/// are informational). Panics loudly on a cyclic or unknown-function call
/// graph, and if any frame exceeds bank 0.
pub fn allocate(m: &Module, edges_text: &str) -> AllocLayout {
    // 1. Globals: sequential, aligned to the type width (i16 -> even address).
    let mut globals: HashMap<String, u8> = HashMap::new();
    let mut addr = GLOBAL_START;
    for g in &m.globals {
        if g.is_const {
            continue;
        }
        let width = g.ty.bytes();
        if addr % width != 0 {
            addr += width - (addr % width);
        }
        globals.insert(g.name.clone(), addr);
        addr += width;
    }

    // end_of_globals = max over the address map of (addr + width), floored at
    // GLOBAL_START (mirrors isel's layout computation); the scratch byte, the
    // two retval bytes and the first frame base follow it.
    let end_of_globals = m.globals.iter().fold(GLOBAL_START, |end, g| {
        match globals.get(&g.name) {
            Some(&a) => end.max(a.wrapping_add(g.ty.bytes())),
            None => end,
        }
    });
    let bank0_start = end_of_globals
        .checked_add(3)
        .expect("alloc: globals end too close to the top of RAM");

    // 2. locals_size(f) = sum of Ty::bytes() over f's params and defined
    // values, each name counted once (phi destinations are defined values;
    // icmp destinations are i1).
    let mut locals_size: HashMap<String, u16> = HashMap::new();
    for f in &m.funcs {
        let mut size: u16 = 0;
        let mut seen: HashSet<String> = HashSet::new();
        for (ty, name) in &f.params {
            if seen.insert(name.clone()) {
                size += u16::from(ty.bytes());
            }
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Some((name, width)) = def_width(inst) {
                    if seen.insert(name) {
                        size += u16::from(width);
                    }
                }
            }
        }
        locals_size.insert(f.name.clone(), size);
    }

    // 3. Call graph from the edge text.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new(); // caller -> callees
    let mut callees: HashSet<String> = HashSet::new();
    for line in edges_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("edge ") {
            let mut it = rest.split_whitespace();
            let caller = it
                .next()
                .unwrap_or_else(|| panic!("alloc: malformed edge line: {line}"))
                .to_string();
            let callee = it
                .next()
                .unwrap_or_else(|| panic!("alloc: malformed edge line: {line}"))
                .to_string();
            assert!(it.next().is_none(), "alloc: malformed edge line: {line}");
            let list = edges.entry(caller).or_default();
            if !list.contains(&callee) {
                list.push(callee.clone());
            }
            callees.insert(callee);
        } else if line.starts_with("depth ") {
            // Informational; ignored.
        } else if line.starts_with("fn ") {
            // The callgraph binary emits one `fn <name>` line per function
            // (after `depth`). It carries no info needed for allocation, so
            // skip it.
        } else {
            panic!("alloc: unrecognized callgraph line: {line}");
        }
    }

    // 4. Topological order (recursion is rejected by callgraph; panic loudly
    // if one slips through, and on any edge to an unknown function).
    let mut indeg: HashMap<String, usize> = m.funcs.iter().map(|f| (f.name.clone(), 0)).collect();
    for (caller, cs) in &edges {
        assert!(
            indeg.contains_key(caller),
            "alloc: edge from unknown function {caller}"
        );
        for c in cs {
            let d = indeg
                .get_mut(c)
                .unwrap_or_else(|| panic!("alloc: edge to unknown function {c}"));
            *d += 1;
        }
    }
    let mut ready: Vec<String> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(f, _)| f.clone())
        .collect();
    ready.sort();
    let mut topo: Vec<String> = Vec::new();
    while let Some(f) = ready.pop() {
        topo.push(f.clone());
        if let Some(cs) = edges.get(&f) {
            for c in cs {
                let d = indeg.get_mut(c).expect("alloc: stale topo edge");
                *d -= 1;
                if *d == 0 {
                    ready.push(c.clone());
                }
            }
        }
    }
    assert!(
        topo.len() == m.funcs.len(),
        "alloc: call graph contains a cycle ({} of {} functions placed)",
        topo.len(),
        m.funcs.len()
    );

    // 5. depth_end(f) = locals_size(f) + max(0, max over callees depth_end(c)).
    // Reverse topo order: every callee precedes its callers.
    let mut depth_end: HashMap<String, u16> = HashMap::new();
    for f in topo.iter().rev() {
        let deepest = edges
            .get(f)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|c| depth_end[c])
            .max()
            .unwrap_or(0);
        depth_end.insert(f.clone(), locals_size[f] + deepest);
    }

    // 6. base(f) = max over direct callers of (base + locals_size); roots at
    // bank0_start. Forward topo order: every caller precedes its callees.
    let mut callers: HashMap<String, Vec<String>> = HashMap::new();
    for (p, cs) in &edges {
        for c in cs {
            callers.entry(c.clone()).or_default().push(p.clone());
        }
    }
    let mut base: HashMap<String, u16> = HashMap::new();
    for f in &topo {
        let b = match callers.get(f) {
            Some(ps) => ps
                .iter()
                .map(|p| base[p] + locals_size[p])
                .max()
                .expect("alloc: empty caller list"),
            None => u16::from(bank0_start),
        };
        base.insert(f.clone(), b);
    }

    // 7. Frame check + local addresses: each local at base(f) + offset, in IR
    // order (params first, then defined values in instruction order).
    let mut locals: HashMap<String, u8> = HashMap::new();
    for f in &m.funcs {
        let b = base[&f.name];
        let size = locals_size[&f.name];
        assert!(
            b + size <= BANK0_END,
            "alloc: frame for {} spans 0x{:02X}..0x{:02X}, exceeds bank 0 (past 0x{:02X})",
            f.name,
            b,
            b + size,
            BANK0_END - 1
        );
        let mut off: u16 = 0;
        let mut seen: HashSet<String> = HashSet::new();
        let mut place = |name: &str, width: u8| {
            if seen.insert(name.to_string()) {
                locals.insert(format!("{}::{name}", f.name), (b + off) as u8);
                off += u16::from(width);
            }
        };
        for (ty, name) in &f.params {
            place(name, ty.bytes());
        }
        for blk in &f.blocks {
            for inst in &blk.insts {
                if let Some((name, width)) = def_width(inst) {
                    place(&name, width);
                }
            }
        }
    }

    // 8. Total bank-0 demand = max over roots of depth_end(root).
    let total_bank0 = m
        .funcs
        .iter()
        .filter(|f| !callees.contains(&f.name))
        .map(|f| depth_end[&f.name])
        .max()
        .unwrap_or(0);

    AllocLayout {
        globals,
        locals,
        total_bank0,
    }
}

/// The value a defining instruction writes: `(name, byte width)`, or `None`
/// for non-defining instructions. `icmp` results are i1 (1 byte) regardless
/// of the operand type.
fn def_width(inst: &Inst) -> Option<(String, u8)> {
    match inst {
        Inst::Load(l) => Some((l.dst.clone(), l.ty.bytes())),
        Inst::Bin(b) => Some((b.dst.clone(), b.ty.bytes())),
        Inst::Zext(z) => Some((z.dst.clone(), z.to.bytes())),
        Inst::Trunc(t) => Some((t.dst.clone(), t.to.bytes())),
        Inst::Icmp(i) => Some((i.dst.clone(), 1)),
        Inst::Select(s) => Some((s.dst.clone(), s.ty.bytes())),
        Inst::Call(c) => match (&c.dst, &c.ty) {
            (Some(d), Some(t)) => Some((d.clone(), t.bytes())),
            _ => None,
        },
        Inst::Phi(p) => Some((p.dst.clone(), p.ty.bytes())),
        Inst::Store(_) | Inst::Ret(_) | Inst::Br(_) | Inst::BrCond(_) => None,
    }
}

/// Render the layout as `global <name> 0xNN` and `local <func> <name> 0xNN`
/// lines, deterministically sorted by key. Consumed by the driver, which keys
/// locals `{func}::{name}`.
pub fn map_text(l: &AllocLayout) -> String {
    let mut out = String::new();
    let mut globals: Vec<&String> = l.globals.keys().collect();
    globals.sort();
    for name in globals {
        out.push_str(&format!("global {name} 0x{:02X}\n", l.globals[name]));
    }
    let mut locals: Vec<&String> = l.locals.keys().collect();
    locals.sort();
    for key in locals {
        let (func, name) = key
            .split_once("::")
            .expect("alloc: malformed local key {key}");
        out.push_str(&format!("local {func} {name} 0x{:02X}\n", l.locals[key]));
    }
    out
}
