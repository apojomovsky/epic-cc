//! Overlay address allocation for the PIC8 pipeline.
//!
//! Globals get sequential, even-aligned (i16) addresses starting at the
//! device's first GPR bank. A bin-packing fallback (largest-first,
//! independent per-bank cursors) activates only when sequential placement
//! would otherwise fail, so every program that already succeeds keeps
//! unchanged addresses. Every local of every function lives in a frame
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
/// last bank has been exhausted). Steps through regions via
/// `device.region_for`, keeping the value even-aligned within its bank
/// region (`align = width.min(2)` — only 2-byte values need even alignment).
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

/// The frame base for a runtime routine: a frame that stays inside the bank
/// its derived base lands in keeps that base (sibling routines pack
/// contiguously, wasting nothing); a frame that would straddle a bank
/// boundary moves wholesale to the next bank's start. The routine recipe
/// loops are skip-sensitive (issue #6): a BANKSEL the banking pass would
/// insert between a test and its target, or between the two operands of a
/// same-skip carry idiom (`INCFSZ f,W` targeting `ADDWF g,F`), would
/// change the skip targets, so the whole frame must live in ONE GPR bank.
fn routine_base(device: &Device, base: u16, widths: &[u8]) -> u16 {
    let end = frame_end(device, base, widths);
    let (_, region_end) = device
        .region_for(base)
        .expect("alloc: routine frame base in a device GPR bank");
    if end - 1 <= region_end {
        return base; // the whole frame fits in the base's bank
    }
    // The frame would straddle: snap to the next bank's start (it always
    // fits there, a routine frame is at most 22 bytes, far under a bank).
    let next = region_end + 1;
    device
        .region_for(next)
        .map(|(s, _)| s)
        .unwrap_or_else(|| panic!("alloc: routine frame needs a bank past 0x{region_end:X}"))
}

/// `base` unchanged for an ordinary function; `routine_base`-rounded for a
/// runtime routine (issue #6). Shared by the main-context and ISR-context
/// base-assignment loops, which both need this same rounding.
fn round_if_routine(device: &Device, f: &str, base: u16, locals_widths: &HashMap<String, Vec<u8>>) -> u16 {
    if ir::is_runtime_routine(f) {
        routine_base(device, base, &locals_widths[f])
    } else {
        base
    }
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
    // so the map text can list them. If sequential placement fails (a small
    // global stranded behind a monotonically-advancing cursor that already
    // moved past a bank with room for it), a largest-first bin-packing
    // fallback with independent per-bank cursors runs below (`.or_else(...)`)
    // before giving up.
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut fixed: Vec<(String, u16, u16)> = Vec::new(); // (name, addr, size)
    let mut floating: Vec<&ir::Global> = Vec::new();
    for g in &m.globals {
        if g.is_const {
            const_globals.insert(g.name.clone());
        } else {
            assert!(g.size <= 255, "alloc: RAM global @{} too large ({} bytes; RAM is byte-addressed, max 255)", g.name, g.size);
            if let Some(a) = g.addr {
                fixed.push((g.name.clone(), a, g.size));
            } else {
                floating.push(g);
            }
        }
    }
    // Floating globals are placed with the same sequential -> bin-pack
    // strategy as before, but pinned addresses are respected and the
    // sequential cursor is bumped past any overlap.
    let mut globals: HashMap<String, u16> = HashMap::new();
    for (name, addr, _) in &fixed {
        globals.insert(name.clone(), *addr);
    }
    let floating_map: HashMap<String, u16> = if floating.is_empty() {
        HashMap::new()
    } else {
        // Sort fixed by address for overlap checks.
        fixed.sort_by_key(|(_, a, _)| *a);
        let try_float = |addr_start: u16| -> Option<HashMap<String, u16>> {
            let mut out = HashMap::new();
            let mut addr = addr_start;
            // Bump past any fixed that covers the start.
            for (_, fa, fs) in &fixed {
                if addr >= *fa && addr < *fa + *fs {
                    addr = *fa + *fs;
                }
            }
            for g in &floating {
                let width = g.size as u8;
                // If the next placement would overlap a fixed region, bump
                // past it before asking try_place_at.
                let mut candidate = addr;
                loop {
                    let mut bumped = false;
                    for (_, fa, fs) in &fixed {
                        if candidate < *fa + *fs && candidate + u16::from(width) > *fa && candidate >= *fa && candidate < *fa + *fs {
                            candidate = *fa + *fs;
                            bumped = true;
                            break;
                        }
                        // Also catch the case where the placed range would
                        // straddle into a fixed region that starts inside it.
                        if candidate < *fa && candidate + u16::from(width) > *fa {
                            // Overlaps the start of a fixed region; bump if
                            // there is overlap, but respect alignment: just
                            // move candidate past the fixed region and retry.
                            candidate = *fa + *fs;
                            bumped = true;
                            break;
                        }
                    }
                    if !bumped {
                        break;
                    }
                }
                let start = try_place_at(device, candidate, width)?;
                // Final overlap check: the aligned placement from
                // try_place_at may have landed inside a fixed region.
                let mut start = start;
                loop {
                    let mut overlap = None;
                    for (_, fa, fs) in &fixed {
                        if start < *fa + *fs && start + u16::from(width) > *fa {
                            overlap = Some(*fa + *fs);
                            break;
                        }
                    }
                    if let Some(next) = overlap {
                        start = try_place_at(device, next, width)?;
                    } else {
                        break;
                    }
                }
                out.insert(g.name.clone(), start);
                addr = start + u16::from(width);
                // Bump addr past any fixed that it now sits inside.
                for (_, fa, fs) in &fixed {
                    if addr > *fa && addr <= *fa + *fs {
                        addr = *fa + *fs;
                    }
                }
            }
            Some(out)
        };
        let seq = {
            let mut start = device.gpr_start();
            for (_, fa, fs) in &fixed {
                if start >= *fa && start < *fa + *fs {
                    start = *fa + *fs;
                }
            }
            try_float(start)
        };
        seq.or_else(|| {
            // Bin-pack fallback: build cursors that skip fixed-occupied
            // bytes. For the small fixtures in this repo the sequential
            // path suffices; this path is kept correct for completeness.
            let mut cursors: Vec<BankCursor> = device
                .ram_banks
                .iter()
                .map(|&(s, e)| BankCursor { end: e, next_free: s })
                .collect();
            // Advance each cursor past any fixed that lies at its start.
            for c in &mut cursors {
                for (_, fa, fs) in &fixed {
                    if c.next_free >= *fa && c.next_free < *fa + *fs {
                        c.next_free = *fa + *fs;
                    }
                }
            }
            let mut order: Vec<&ir::Global> = floating.clone();
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
                    // Skip fixed overlap inside this bank.
                    let mut bumped = true;
                    while bumped {
                        bumped = false;
                        for (_, fa, fs) in &fixed {
                            if base < *fa + *fs && base + u16::from(width) > *fa {
                                base = *fa + *fs;
                                if base % u16::from(align) != 0 {
                                    base += u16::from(align) - (base % u16::from(align));
                                }
                                bumped = true;
                                break;
                            }
                        }
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
        })
        .unwrap_or_else(|| {
            let demand: u32 = floating.iter().map(|g| u32::from(g.size)).sum::<u32>()
                + fixed.iter().map(|(_, _, s)| u32::from(*s)).sum::<u32>();
            let capacity: u32 =
                device.ram_banks.iter().map(|&(s, e)| u32::from(e) - u32::from(s) + 1).sum();
            let bank_count = device.ram_banks.len();
            panic!(
                "alloc: no arrangement of {} global(s) fits {}'s {bank_count} GPR bank window(s) \
                 (total demand {demand} bytes, total capacity {capacity} bytes — no arrangement this \
                 allocator tries, sequential then largest-first bin-packing, fits)",
                floating.len() + fixed.len(),
                device.name,
            );
        })
    };
    globals.extend(floating_map);

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
        // Issue #6: a runtime routine's frame must stay inside ONE GPR bank
        // (its skip-sensitive recipe loops cannot tolerate a BANKSEL between
        // a test and its target, or inside a carry idiom). The base is
        // rounded when the derived frame would straddle a bank boundary.
        let b = round_if_routine(device, f, b, &locals_widths);
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
            // Issue #6: the ISR context's routine copies get the same
            // single-bank frame rounding as the main context's.
            let b = round_if_routine(device, f, b, &locals_widths);
            base.insert(f.clone(), b);
        }
    }

    // 7. Local addresses: each local at the next free frame byte, in IR
    // order (params first, then defined values in instruction order), stepping
    // through the banks via `place_contiguous`/`region_for`, which panics if
    // a frame exceeds the device's last GPR bank (0x1EF on PIC16F877A).
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
        // Asm is opaque verbatim, defines no SSA value needing a RAM slot.
        Inst::Asm(_) => None,
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

    #[test]
    fn try_place_at_returns_none_instead_of_panicking_past_the_last_bank() {
        // PIC16F877A's last bank ends at 0x1EF; nothing at or past 0x1F0 has
        // a region, so placing even a 1-byte value there must fail cleanly.
        assert_eq!(try_place_at(&PIC16F877A, 0x1F0, 1), None);
    }

}
