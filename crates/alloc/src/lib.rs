//! Overlay address allocation for the PIC8 pipeline.
//!
//! Globals get sequential, even-aligned (i16) addresses starting at the
//! device's first GPR bank. Every local of every function lives in a frame
//! assigned from the call graph: `base(f) = max over callers of the
//! caller's **physical** frame end` (the address just past its last placed
//! local, bank crossings included — see `frame_end`), roots start after the
//! globals, so sibling functions (never co-live) share RAM.
//!
//! Both allocators assign **physical** addresses and step through the
//! device's GPR banks (`Device::region_for`); demand past the last bank
//! panics. The device's common RAM is never used by locals (M3 decision) —
//! the bank progression jumps past it — and holds the fixed scratch/retval
//! bytes instead (see `isel`).

use std::collections::{HashMap, HashSet};

use device::Device;
use ir::{Inst, Module};

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
/// Common RAM, SFRs, and any unimplemented gap fall into the next bank's
/// range, so locals never land there. Panics past the device's last bank.
fn region_for(device: &Device, addr: u16) -> (u16, u16) {
    device.region_for(addr).unwrap_or_else(|| {
        let last_end = device.ram_banks.last().expect("a device has at least one GPR bank").1;
        panic!("alloc: GPR demand exceeds 0x{last_end:X} ({addr:#06x})")
    })
}

/// The start address for a `width`-byte value placed at the next free
/// address `addr`, or `None` if no region past `addr` has room (the device's
/// last bank has been exhausted). Same placement rule as `place_at` — step
/// through regions, `align = width.min(2)` — without the panic.
fn try_place_at(device: &Device, addr: u16, width: u8) -> Option<u16> {
    let align = width.min(2);
    let mut a = addr;
    loop {
        let (start, end) = device.region_for(a)?;
        let mut base = a.max(start);
        if base % u16::from(align) != 0 {
            base += u16::from(align) - (base % u16::from(align));
        }
        if base + u16::from(width) - 1 <= end {
            return Some(base);
        }
        a = end + 1;
    }
}

/// The start address for a `width`-byte value placed at the next free address
/// `addr`: step across banks when `addr` has passed a region's end, keep the
/// value even-aligned within its bank region (only 2-byte values need even
/// alignment; larger arrays advance sequentially), and panic past the
/// device's last bank.
fn place_at(device: &Device, addr: u16, width: u8) -> u16 {
    // Only i16/2-byte globals need even alignment; larger arrays advance
    // sequentially (min(size, 2) keeps a multi-byte value from being padded
    // out to a multiple of its own width, which would waste RAM).
    let align = width.min(2);
    let mut a = addr;
    loop {
        let (start, end) = region_for(device, a);
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

/// `globals` placed in order with ONE monotonically-advancing cursor —
/// exactly `allocate()`'s original globals loop, extracted so it can be
/// tried before falling back to bin-packing. Returns `None` the first time
/// any global doesn't fit, rather than panicking.
fn try_place_globals_sequential(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>> {
    let mut out = HashMap::new();
    let mut addr: u16 = device.gpr_start();
    for g in globals {
        let width = g.size as u8;
        let start = try_place_at(device, addr, width)?;
        out.insert(g.name.clone(), start);
        addr = start + u16::from(width);
    }
    Some(out)
}

/// The start address for a `width`-byte local placed contiguously at the next
/// free frame byte `addr`: step across banks when `addr` has passed a
/// region's end, and panic past the device's last bank. Locals are NOT
/// even-aligned — the overlay frame math (M3) is a plain byte sum, so placing
/// locals contiguously keeps a frame's virtual footprint exactly equal to
/// `locals_size(f)`, and a caller's physical end (see `frame_end`) is the
/// address its callees' bases are derived from. The bank progression starts
/// every region on an even address, so i16s placed there are naturally
/// even-aligned within each bank.
fn place_contiguous(device: &Device, addr: u16, width: u8) -> u16 {
    let mut a = addr;
    loop {
        let (start, end) = region_for(device, a);
        let base = a.max(start);
        if base + u16::from(width) - 1 <= end {
            return base;
        }
        // The local doesn't fit in this region; continue past its end.
        a = end + 1;
    }
}

/// One bank's independently-tracked free-space frontier during bin-packing.
struct BankCursor {
    end: u16,
    next_free: u16,
}

/// Places `globals` largest-first into whichever bank's free-space cursor
/// has room first (First-Fit-Decreasing), so a small global declared after
/// several large ones can still land in an earlier bank's leftover space —
/// unlike `try_place_globals_sequential`'s single monotonically-advancing
/// cursor, which abandons every bank's leftover the moment it moves on.
/// Returns `None` only if some global has no room in any bank once every
/// earlier (larger-or-equal) global has been placed.
fn place_globals_bin_packed(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>> {
    let mut cursors: Vec<BankCursor> =
        device.ram_banks.iter().map(|&(start, end)| BankCursor { end, next_free: start }).collect();
    let mut order: Vec<&ir::Global> = globals.to_vec();
    order.sort_by(|a, b| b.size.cmp(&a.size));

    let mut out = HashMap::new();
    for g in order {
        let width = g.size as u8;
        let align = width.min(2);
        let mut placed = None;
        for cursor in cursors.iter_mut() {
            let mut base = cursor.next_free;
            if base % u16::from(align) != 0 {
                base += u16::from(align) - (base % u16::from(align));
            }
            if base + u16::from(width) - 1 <= cursor.end {
                cursor.next_free = base + u16::from(width);
                placed = Some(base);
                break;
            }
        }
        out.insert(g.name.clone(), placed?);
    }
    Some(out)
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
fn frame_end(device: &Device, base: u16, widths: &[u8]) -> u16 {
    let mut addr = base;
    for &w in widths {
        addr = place_contiguous(device, addr, w) + u16::from(w);
    }
    addr
}

/// Every function transitively reachable from `roots` over the caller ->
/// callee map `edges` (the roots included). A visited set keeps a call cycle
/// (rejected loudly earlier by the topological sort) from looping forever.
fn reachable<'a>(roots: &[&'a str], edges: &'a HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<&str> = roots.to_vec();
    while let Some(f) = stack.pop() {
        if !seen.insert(f.to_string()) {
            continue;
        }
        if let Some(cs) = edges.get(f) {
            for c in cs {
                if !seen.contains(c) {
                    stack.push(c);
                }
            }
        }
    }
    seen
}

/// Assign every address: globals sequential (even-aligned i16, stepping
/// through banks), locals per the overlay algorithm over the call graph parsed
/// from `edges_text` (`edge <caller> <callee>` lines, order-agnostic; `depth`
/// lines are informational). Panics loudly on a cyclic or unknown-function
/// call graph, and if total demand exceeds the device's GPR space.
pub fn allocate(device: &Device, m: &Module, edges_text: &str) -> AllocLayout {
    // 1. Globals: sequential, aligned to at most two bytes (i16 -> even
    // address; larger arrays advance sequentially), stepping through the banks as bank 0 GPR fills up. Each global spans
    // `size` bytes (an `[N x T]` array takes N addresses, not one), so a
    // sized array advances the free pointer by its byte count. Const globals
    // get no RAM address (their bytes live in flash) but are still recorded
    // so the map text can list them.
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut non_const: Vec<&ir::Global> = Vec::new();
    for g in &m.globals {
        if g.is_const {
            const_globals.insert(g.name.clone());
        } else {
            assert!(g.size <= 255, "alloc: RAM global @{} too large ({} bytes; RAM is byte-addressed, max 255)", g.name, g.size);
            non_const.push(g);
        }
    }
    let globals: HashMap<String, u16> = try_place_globals_sequential(device, &non_const)
        .unwrap_or_else(|| {
            let last_end = device.ram_banks.last().expect("a device has at least one GPR bank").1;
            panic!("alloc: GPR demand exceeds 0x{last_end:X}")
        });

    // end_of_globals = max over the address map of (addr + width), floored at
    // the device's GPR start (mirrors isel's layout computation). The
    // scratch/retval bytes live in the device's fixed common RAM, so the
    // first frame base follows the globals directly.
    let end_of_globals = m.globals.iter().fold(device.gpr_start(), |end, g| {
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
                .map(|p| frame_end(device, base[p], &locals_widths[p]))
                .max()
                .expect("alloc: empty caller list"),
            None => bank0_start,
        };
        base.insert(f.clone(), b);
    }

    // 6b. The disjoint ISR region: an ISR root's frame base is AFTER the
    // main context's total (the max physical frame end over the NON-ISR
    // roots' contexts), not `bank0_start` — the ISR can preempt main at any
    // point, so a preempted main's live frames must never overlap the ISR
    // context's frames. The plan states this as "max depth_end over the
    // NON-ISR roots"; the physical variant equals that when no local crosses
    // a bank gap and is strictly larger (hence safer) when an i16 at a
    // region tail leaves a hole, exactly the frame_end vs depth_end
    // distinction the overlay already makes for callee bases. The main loop
    // above is exact for every non-ISR context (no ISR-side function is
    // reachable from them), so the disjoint base is computed from its
    // results; the ISR contexts are then re-derived from that base in topo
    // order (callers precede callees, and every caller of an ISR-context
    // function is itself in the ISR context after the legalize duplication).
    let isr_names: HashSet<&str> = m.funcs.iter().filter(|f| f.isr).map(|f| f.name.as_str()).collect();
    if !isr_names.is_empty() {
        let isr_roots: Vec<&String> = topo
            .iter()
            .filter(|f| !callers.contains_key(*f) && isr_names.contains(f.as_str()))
            .collect();
        let non_isr_roots: Vec<&String> = topo
            .iter()
            .filter(|f| !callers.contains_key(*f) && !isr_names.contains(f.as_str()))
            .collect();
        // The disjoint base = max physical frame end over the non-ISR roots'
        // contexts (the main context's total). No non-ISR root: the ISR is
        // the only root, so it simply starts at bank0_start.
        let isr_base = non_isr_roots
            .iter()
            .flat_map(|r| reachable(&[r.as_str()], &edges))
            .map(|f| frame_end(device, base[&f], &locals_widths[&f]))
            .max()
            .unwrap_or(bank0_start);
        // Re-derive the ISR contexts from the disjoint base (topo order: an
        // ISR root's base is fixed first, then each callee's base derives
        // from its already-fixed callers).
        let isr_ctx: HashSet<String> =
            isr_roots.iter().flat_map(|r| reachable(&[r.as_str()], &edges)).collect();
        for f in &topo {
            if !isr_ctx.contains(f) {
                continue;
            }
            let b = if isr_names.contains(f.as_str()) {
                isr_base
            } else {
                callers[f]
                    .iter()
                    .map(|p| frame_end(device, base[p], &locals_widths[p]))
                    .max()
                    .expect("alloc: empty caller list")
            };
            base.insert(f.clone(), b);
        }
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
                let start = place_contiguous(device, addr, width);
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

    // 8. Total bank-0 demand = max over roots of depth_end(root), with an
    // ISR root's disjoint base offset included (its region starts after the
    // main context's total, not at bank0_start).
    let total_bank0 = m
        .funcs
        .iter()
        .filter(|f| !callees.contains(&f.name))
        .map(|f| depth_end[&f.name] + base[&f.name] - bank0_start)
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
        // Freeze defines the dst slot, sized by the operand type.
        Inst::Freeze(f) => Some((f.dst.clone(), f.ty.bytes())),
        // Float: binops/conv casts define an f32 (4-byte) dst; fcmp an i1.
        Inst::FloatBin(b) => Some((b.dst.clone(), 4)),
        Inst::Fcmp(c) => Some((c.dst.clone(), 1)),
        Inst::FloatConv(c) => Some((c.dst.clone(), c.to.bytes())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use device::PIC16F877A;

    fn global(name: &str, size: u16) -> ir::Global {
        ir::Global { name: name.to_string(), ty: ir::Ty::I8, is_const: false, size, bytes: Vec::new(), addr: None }
    }

    #[test]
    fn try_place_at_returns_none_instead_of_panicking_past_the_last_bank() {
        // PIC16F877A's last bank ends at 0x1EF; nothing at or past 0x1F0 has
        // a region, so placing even a 1-byte value there must fail cleanly.
        assert_eq!(try_place_at(&PIC16F877A, 0x1F0, 1), None);
    }

    #[test]
    fn try_place_globals_sequential_returns_none_when_a_later_global_cannot_fit_anywhere() {
        // Three 76-byte globals, one 78-byte global, then one 4-byte global
        // (310 bytes total, well under the device's 320-byte capacity) — the
        // single advancing cursor abandons a 4-byte leftover in each of the
        // first three banks it uses, then the 78-byte global leaves only 2
        // bytes in the fourth (last) bank, too little for the trailing
        // 4-byte global with nowhere left to go. See Task 3's integration
        // test for the full derivation of these exact sizes.
        let g0 = global("g0", 76);
        let g1 = global("g1", 76);
        let g2 = global("g2", 76);
        let g3 = global("g3", 78);
        let g4 = global("g4", 4);
        let refs: Vec<&ir::Global> = vec![&g0, &g1, &g2, &g3, &g4];
        assert_eq!(try_place_globals_sequential(&PIC16F877A, &refs), None);
    }

    #[test]
    #[should_panic(expected = "0x01f0")]
    fn place_at_panic_message_shows_stepped_cursor_not_original_arg() {
        // PIC16F877A's last bank ends at 0x1EF. Trying to place a 2-byte
        // value at 0x1EF requires even alignment, so it steps to 0x1F0, which
        // is past the last bank. The panic message must show the stepped
        // cursor (0x1f0), not the original argument (0x1ef). This regression
        // test verifies the fix for the bug where delegating to try_place_at
        // would incorrectly show the original argument.
        let _ = place_at(&PIC16F877A, 0x1EF, 2);
    }

    #[test]
    fn bin_packed_places_all_globals_with_no_overlaps_and_within_one_bank_each() {
        // Same reproduction input as Task 1's sequential-failure test: three
        // 76-byte globals, one 78-byte global, one 4-byte global (310 bytes
        // total). Bin-packing succeeds where the single advancing cursor
        // does not (see Task 3's integration test, which proves the
        // sequential-fails / bin-packed-succeeds contrast through the full
        // `allocate()` entry point).
        let g0 = global("g0", 76);
        let g1 = global("g1", 76);
        let g2 = global("g2", 76);
        let g3 = global("g3", 78);
        let g4 = global("g4", 4);
        let refs: Vec<&ir::Global> = vec![&g0, &g1, &g2, &g3, &g4];
        let placed = place_globals_bin_packed(&PIC16F877A, &refs).expect("bin-packing must succeed");
        assert_eq!(placed.len(), 5);

        // No two globals may overlap, and every placement must lie fully
        // within a single bank's inclusive range (never straddling one).
        let mut spans: Vec<(u16, u16)> = refs
            .iter()
            .map(|g| {
                let start = placed[&g.name];
                (start, start + g.size - 1)
            })
            .collect();
        for &(start, end) in &spans {
            assert!(
                PIC16F877A.ram_banks.iter().any(|&(bs, be)| start >= bs && end <= be),
                "global at 0x{start:03X}..=0x{end:03X} does not fit inside a single bank"
            );
        }
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 < w[1].0, "overlapping placements: {:?} and {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn bin_packed_returns_none_when_total_demand_truly_exceeds_capacity() {
        // 5 objects of 70 bytes each = 350 bytes > the device's 320-byte
        // total GPR capacity (4 banks x 80 bytes) — no arrangement fits.
        let gs: Vec<ir::Global> = (0..5).map(|i| global(&format!("g{i}"), 70)).collect();
        let refs: Vec<&ir::Global> = gs.iter().collect();
        assert_eq!(place_globals_bin_packed(&PIC16F877A, &refs), None);
    }
}
