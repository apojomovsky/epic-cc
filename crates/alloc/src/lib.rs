//! Overlay address allocation for the PIC8 pipeline.
//!
//! Globals get sequential, even-aligned (i16) addresses starting at
//! `GLOBAL_START`. Every local of every function lives in a frame assigned
//! from the call graph: `base(f) = max over callers of the caller's
//! **physical** frame end` (the address just past its last placed local,
//! bank crossings included — see `frame_end`), roots start after the
//! globals, so sibling functions (never co-live) share RAM.
//!
//! Both allocators assign **physical** addresses and step through the four
//! banks: bank 0 GPR `0x20-0x6F`, bank 1 `0xA0-0xEF`, bank 2 `0x120-0x16F`,
//! bank 3 `0x1A0-0x1EF` (`0x190-0x19F` is unimplemented RAM); demand past
//! `0x1EF` panics. Common RAM (`0x70-0x7F`) is never used by locals (M3
//! decision) — the bank progression jumps past it — and holds the fixed
//! scratch/retval bytes instead.

use std::collections::{HashMap, HashSet};

use ir::{Inst, Module};

pub const GLOBAL_START: u8 = 0x20;

/// Complete address map: globals keyed by name, locals keyed `{func}::{name}`,
/// plus the total overlay span (in bytes) across all banks. `const_globals`
/// lists the names of const globals (no RAM address; their bytes live in
/// flash), so the map text can emit `const <name>` lines for them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AllocLayout {
    pub globals: HashMap<String, u16>,
    pub locals: HashMap<String, u16>,
    pub total_bank0: u16,
    pub const_globals: HashSet<String>,
}

/// Inclusive physical-address range of the GPR region that contains `addr`,
/// advancing to the next bank when `addr` has spilled past the current one.
/// Common RAM (`0x70-0x7F`), SFRs, and the unimplemented gap
/// (`0x170-0x19F`) fall into the next bank's range, so locals never land
/// there.
fn region_for(addr: u16) -> (u16, u16) {
    if addr <= 0x6F {
        (0x20, 0x6F)
    } else if addr <= 0xEF {
        (0xA0, 0xEF)
    } else if addr <= 0x16F {
        (0x120, 0x16F)
    } else if addr <= 0x1EF {
        (0x1A0, 0x1EF)
    } else {
        panic!("alloc: GPR demand exceeds 0x1EF ({addr:#06x})");
    }
}

/// The start address for a `width`-byte value placed at the next free address
/// `addr`: step across banks when `addr` has passed a region's end, keep the
/// value even-aligned within its bank region (only 2-byte values need even
/// alignment; larger arrays advance sequentially), and panic past `0x1EF`.
fn place_at(addr: u16, width: u8) -> u16 {
    // Only i16/2-byte globals need even alignment; larger arrays advance
    // sequentially (min(size, 2) keeps a multi-byte value from being padded
    // out to a multiple of its own width, which would waste RAM).
    let align = width.min(2);
    let mut a = addr;
    loop {
        let (start, end) = region_for(a);
        let mut base = a.max(start);
        if base % u16::from(align) != 0 {
            base += u16::from(align) - (base % u16::from(align));
        }
        if base + u16::from(width) - 1 <= end {
            return base;
        }
        // The value doesn't fit in this region; continue just past its end.
        a = end + 1;
    }
}

/// The start address for a `width`-byte local placed contiguously at the next
/// free frame byte `addr`: step across banks when `addr` has passed a
/// region's end, and panic past `0x1EF`. Locals are NOT even-aligned — the
/// overlay frame math (M3) is a plain byte sum, so placing locals contiguously
/// keeps a frame's virtual footprint exactly equal to `locals_size(f)`, and a
/// caller's physical end (see `frame_end`) is the address its callees' bases
/// are derived from. The bank progression starts every region on an even
/// address, so i16s placed there are naturally even-aligned within each bank.
fn place_contiguous(addr: u16, width: u8) -> u16 {
    let mut a = addr;
    loop {
        let (start, end) = region_for(a);
        let base = a.max(start);
        if base + u16::from(width) - 1 <= end {
            return base;
        }
        // The local doesn't fit in this region; continue past its end.
        a = end + 1;
    }
}

/// The physical address just past a frame whose locals, in placement order
/// (`widths`), are placed contiguously at `base` — i.e. the final address
/// after walking each local through `place_contiguous`, exactly the way the
/// locals are laid out. This is the address a callee overlaid on this frame
/// must be derived from. A plain contiguous-blob model (`base + total_size`,
/// stepping through the regions) would under-count the end: a local that does
/// not fit in the bytes left in a region moves *wholesale* to the next region,
/// leaving the region-tail byte unused (a 1-byte hole whenever an i16 local is
/// placed at 0x6F/0xEF/0x16F), so the true end can trail the blob's end by one
/// byte per crossing — and a callee based on the blob end could land exactly
/// on the caller's live locals. Walking the actual placements reproduces the
/// layout step that assigns the locals, so the result is the true physical
/// end. When no local crosses a gap the walk reduces to `base + total_size`,
/// keeping existing non-crossing layouts unchanged.
fn frame_end(base: u16, widths: &[u8]) -> u16 {
    let mut addr = base;
    for &w in widths {
        addr = place_contiguous(addr, w) + u16::from(w);
    }
    addr
}

/// Assign every address: globals sequential (even-aligned i16, stepping
/// through banks), locals per the overlay algorithm over the call graph parsed
/// from `edges_text` (`edge <caller> <callee>` lines, order-agnostic; `depth`
/// lines are informational). Panics loudly on a cyclic or unknown-function
/// call graph, and if total demand exceeds `0x1EF`.
pub fn allocate(m: &Module, edges_text: &str) -> AllocLayout {
    // 1. Globals: sequential, aligned to at most two bytes (i16 -> even
    // address; larger arrays advance sequentially), stepping through the banks as bank 0 GPR fills up. Each global spans
    // `size` bytes (an `[N x T]` array takes N addresses, not one), so a
    // sized array advances the free pointer by its byte count. Const globals
    // get no RAM address (their bytes live in flash) but are still recorded
    // so the map text can list them.
    let mut globals: HashMap<String, u16> = HashMap::new();
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut addr: u16 = u16::from(GLOBAL_START);
    for g in &m.globals {
        if g.is_const {
            const_globals.insert(g.name.clone());
            continue;
        }
        let width = g.size;
        let start = place_at(addr, width);
        globals.insert(g.name.clone(), start);
        addr = start + u16::from(width);
    }

    // end_of_globals = max over the address map of (addr + width), floored at
    // GLOBAL_START (mirrors isel's layout computation). The scratch/retval
    // bytes now live in fixed common RAM (0x70-0x72), so the first frame base
    // follows the globals directly.
    let end_of_globals = m.globals.iter().fold(u16::from(GLOBAL_START), |end, g| {
        match globals.get(&g.name) {
            Some(&a) => end.max(a + u16::from(g.size)),
            None => end,
        }
    });
    let bank0_start = end_of_globals;

    // 2. locals_widths(f) = the byte widths of f's params and defined values
    // in placement order (params first, then defined values in instruction
    // order), each name counted once (phi destinations are defined values;
    // icmp destinations are i1). The physical frame end (step 6) is derived
    // by walking these widths through `place_contiguous`, and locals_size(f)
    // (the virtual footprint, used for depth_end) is their sum.
    let mut locals_widths: HashMap<String, Vec<u8>> = HashMap::new();
    for f in &m.funcs {
        let mut widths: Vec<u8> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for p in &f.params {
            if seen.insert(p.name.clone()) {
                widths.push(p.width);
            }
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Some((name, width)) = def_width(inst) {
                    if seen.insert(name) {
                        widths.push(width);
                    }
                }
            }
        }
        locals_widths.insert(f.name.clone(), widths);
    }
    let locals_size: HashMap<String, u16> = locals_widths
        .iter()
        .map(|(f, ws)| (f.clone(), ws.iter().map(|&w| u16::from(w)).sum()))
        .collect();

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

    // 6. base(f) = max over direct callers of the caller's PHYSICAL frame
    // end (the address just past its last *placed* local, bank crossings and
    // region-tail holes included); roots at bank0_start. Forward topo order:
    // every caller precedes its callees. The virtual sum base(p) +
    // locals_size[p] is NOT used: a caller whose frame spills past a bank
    // region end ends beyond that sum, and a callee based on it could land in
    // the gap at the next region's start — exactly where the caller's spill
    // locals live while both frames are live. frame_end walks the caller's
    // actual local widths through place_contiguous, so it matches the layout
    // step's placement exactly (including the unused hole byte an i16 leaves
    // when it cannot fit in the region tail).
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
                .map(|p| frame_end(base[p], &locals_widths[p]))
                .max()
                .expect("alloc: empty caller list"),
            None => bank0_start,
        };
        base.insert(f.clone(), b);
    }

    // 7. Local addresses: each local at the next free frame byte, in IR
    // order (params first, then defined values in instruction order), stepping
    // through the banks. `place_at` panics if a frame exceeds 0x1EF.
    let mut locals: HashMap<String, u16> = HashMap::new();
    for f in &m.funcs {
        let b = base[&f.name];
        let mut addr = b;
        let mut seen: HashSet<String> = HashSet::new();
        let mut place = |name: &str, width: u8| {
            if seen.insert(name.to_string()) {
                let start = place_contiguous(addr, width);
                locals.insert(format!("{}::{name}", f.name), start);
                addr = start + u16::from(width);
            }
        };
        for p in &f.params {
            place(&p.name, p.width);
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
        const_globals,
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
        Inst::Sext(s) => Some((s.dst.clone(), s.to.bytes())),
        Inst::Trunc(t) => Some((t.dst.clone(), t.to.bytes())),
        Inst::Icmp(i) => Some((i.dst.clone(), 1)),
        Inst::Select(s) => Some((s.dst.clone(), s.ty.bytes())),
        Inst::Call(c) => match (&c.dst, &c.ty) {
            (Some(d), Some(t)) => Some((d.clone(), t.bytes())),
            _ => None,
        },
        Inst::Phi(p) => Some((p.dst.clone(), p.ty.bytes())),
        Inst::Store(_) | Inst::Ret(_) | Inst::Br(_) | Inst::BrCond(_) => None,
        // Gep computes a virtual pointer address (isel turns it into FSR/INDF
        // or a RETLW table read); it defines no value needing a RAM slot.
        Inst::Gep(_) => None,
        // Alloca defines a size-byte local buffer (the slot is sized below
        // and in alloc); Memcpy defines nothing.
        Inst::Alloca(a) => Some((a.dst.clone(), a.size)),
        Inst::Memcpy(_) => None,
    }
}

/// Render the layout as `global <name> 0xNN`, `const <name>` (no address —
/// the global lives in flash), and `local <func> <name> 0xNN` lines,
/// deterministically sorted by key. Consumed by the driver, which keys
/// locals `{func}::{name}` and distinguishes const globals via the `const`
/// prefix (isel reads their bytes from flash, never from a RAM slot).
pub fn map_text(l: &AllocLayout) -> String {
    let mut out = String::new();
    let mut globals: Vec<&String> = l.globals.keys().collect();
    globals.sort();
    for name in globals {
        out.push_str(&format!("global {name} 0x{:02X}\n", l.globals[name]));
    }
    let mut consts: Vec<&String> = l.const_globals.iter().collect();
    consts.sort();
    for name in consts {
        out.push_str(&format!("const {name}\n"));
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
