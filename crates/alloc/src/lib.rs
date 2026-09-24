//! Overlay address allocation for the PIC8 pipeline.
//!
//! Globals get sequential, even-aligned (i16) addresses. A bin-packing
//! fallback (largest-first, independent per-bank cursors) activates only
//! when sequential placement would otherwise fail, so every program that
//! already succeeds keeps unchanged addresses. Every local of every
//! function lives in a frame assigned from the call graph: `base(f) = max
//! over callers of the caller's **physical** frame end` (the address just
//! past its last placed local, bank crossings included; see `frame_end`),
//! so sibling functions (never co-live) share RAM.
//!
//! Which block starts at the device's first GPR bank is the core's call:
//! PIC14 puts the globals there and the frames above them, PIC18 the
//! reverse. Only PIC18's low RAM (the access bank) is reachable with no
//! bank select, and frames are what direct file-register operands name
//! (epic-cc#482).
//!
//! Both allocators assign **physical** addresses and step through the
//! device's GPR banks (`Device::region_for`); demand past the last bank
//! panics. The liveness overlay never places locals in common RAM (the bank
//! progression jumps past it); common RAM holds the fixed scratch and
//! retval bytes instead (see `isel`).

use std::collections::{HashMap, HashSet};

use device::Device;
use ir::{Inst, Module};
use iselcore::{resolve_pointers, ssa_key, Base, PtrResolution};

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
    /// Per-bank high-water bytes (both main and ISR contexts): the highest
    /// allocated address in each GPR bank minus the bank start, floored at
    /// 0. The allocator places sequentially from each bank start, so this
    /// is the occupied bytes; the only holes are the 1-byte region-tail
    /// gaps an i16 leaves when it moves wholesale to the next bank, which
    /// the high-water mark conservatively includes.
    pub bank_used: Vec<u16>,
    /// The disjoint ISR region's span in bytes (0 without an ISR): the
    /// distance from the ISR root's base to the highest ISR-context frame
    /// end. Reported separately and included in `bank_used`.
    pub isr_bytes: u16,
    /// Whether the module has an ISR. Distinct from `isr_bytes > 0`: a
    /// store-only ISR (e.g. a flag-clear handler) has no local frames, so
    /// its overlay region span is 0, but the backend still emits the
    /// ISR-save prologue, which the size report must count.
    pub has_isr: bool,
    /// The low-priority ISR's 19-byte context-save area base (`Some` only
    /// in priority mode: both a high- and a low-priority ISR exist). It
    /// sits at the low overlay region's base, below the low frames, so it
    /// is disjoint from every context by construction; the high ISR keeps
    /// the device's fixed save block. `None` in compatibility mode (zero
    /// or one ISR), where the single handler uses the fixed block.
    pub isr_low_save: Option<u16>,
    /// The compatibility-mode ISR's 7-byte area for the PROD/FSR1/TABLAT
    /// and PCLATH/PCLATU save bytes (`Some` only when the module has an
    /// ISR, is PIC18, and does not run priority mode). Carved from the
    /// ISR context's own region, like `isr_low_save`, because the fixed
    /// access-bank block is pinned at 16 bytes by `ram_banks` starting at
    /// 0x0010 on every device and has no room for them. (epic-cc#477,
    /// epic-cc#641)
    pub isr_save: Option<u16>,
    /// The high-priority ISR's 7-byte area for the same bytes, carved from
    /// its own region so the two ISRs' areas stay disjoint. `Some` only in
    /// priority mode. (epic-cc#477, epic-cc#641)
    pub isr_hi_save: Option<u16>,
}

/// Inclusive physical-address range of the GPR region that contains `addr`,
/// advancing to the next bank when `addr` has spilled past the current one.
/// Common RAM, SFRs, and any unimplemented gap fall into the next bank's
/// range, so locals never land there. Panics past the device's last bank.
fn region_for(device: &Device, addr: u16) -> (u16, u16) {
    device.region_for(addr).unwrap_or_else(|| {
        let last_end = device
            .ram_banks
            .last()
            .expect("a device has at least one GPR bank")
            .1;
        panic!("alloc: GPR demand exceeds 0x{last_end:X} ({addr:#06x})")
    })
}

/// The physical address just past a `width`-byte global placed at `start`:
/// the address a following global's cursor must advance to. For a
/// single-bank object this is `start + width`; for a bank-straddling object
/// (PIC14E, docs/33 §D-2) the bytes skip common RAM, so the physical end is
/// `start + width` plus the skipped common-RAM bytes. Mirrors the placement
/// walk in `try_place_straddle`.
fn physical_end(device: &Device, start: u16, width: u16) -> u16 {
    let mut cur = start;
    let mut remaining = width;
    while remaining > 0 {
        let (region_start, end) = device
            .region_for(cur)
            .expect("alloc: global placement past the last GPR bank");
        if cur < region_start {
            cur = region_start; // skip common RAM / unimplemented gap
        }
        let avail = end - cur + 1;
        if remaining <= avail {
            return cur + remaining;
        }
        remaining -= avail;
        cur = end + 1;
    }
    cur
}

/// The start address for a `width`-byte value placed at the next free
/// address `addr`, or `None` if no region past `addr` has room (the device's
/// last bank has been exhausted). Steps through regions via
/// `device.region_for`, keeping the value even-aligned within its bank
/// region (`align = width.min(2)`; only 2-byte values need even alignment).
///
/// On PIC14E a global too large for any single GPR bank may straddle a bank
/// boundary (docs/33 §D-2): the object's bytes are placed contiguously
/// through the GPR banks, skipping common RAM, and `isel-pic14e` addresses
/// it through the linear region so one FSR walks across banks. On every
/// other core a straddling global is unrepresentable (classic PIC14's
/// FSR+IRP cannot cross a bank), so it panics.
fn try_place_at(device: &Device, addr: u16, width: u16) -> Option<u16> {
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
        if device.core == device::Core::Pic14e && u16::from(width) > end - start + 1 {
            // Too large for one region: straddle across banks (PIC14E only).
            return try_place_straddle(device, base, width);
        }
        a = end + 1;
    }
}

/// Place a `width`-byte global contiguously starting at the aligned `addr`,
/// stepping across GPR bank boundaries (skipping common RAM) so the object
/// may straddle two banks. Returns the start address, or `None` past the
/// device's last bank. The object's bytes all land in GPR banks; the
/// physical layout is non-contiguous (the common-RAM hole between banks is
/// skipped), which is exactly the case `isel-pic14e` addresses through the
/// linear region (docs/33 §D-2).
fn try_place_straddle(device: &Device, addr: u16, width: u16) -> Option<u16> {
    let mut cur = addr;
    let mut remaining = u16::from(width);
    while remaining > 0 {
        let (start, end) = device.region_for(cur)?;
        if cur < start {
            cur = start; // skip common RAM / unimplemented gap
        }
        let avail = end - cur + 1;
        if remaining <= avail {
            return Some(addr);
        }
        remaining -= avail;
        cur = end + 1;
    }
    Some(addr)
}

/// The start address for a `width`-byte local placed contiguously at the next
/// free frame byte `addr`: step across banks when `addr` has passed a
/// region's end, and panic past the device's last bank. Locals are NOT
/// even-aligned: the liveness-overlay frame math is a plain byte sum, so placing
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
/// (`widths`), are placed contiguously at `base`: the final address
/// after walking each local through `place_contiguous`, exactly the way the
/// locals are laid out. This is the address a callee overlaid on this frame
/// must be derived from. A plain contiguous-blob model (`base + total_size`,
/// stepping through the regions) would under-count the end: a local that does
/// not fit in the bytes left in a region moves *wholesale* to the next region,
/// leaving the region-tail byte unused (a 1-byte hole whenever an i16 local is
/// placed at 0x6F/0xEF/0x16F), so the true end can trail the blob's end by one
/// byte per crossing, and a callee based on the blob end could land exactly
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
/// boundary moves wholesale to the next bank's start. The recipe loops are
/// skip-sensitive: a BANKSEL between a test and its target, or between the
/// two operands of a same-skip carry idiom, would change the skip targets,
/// so the whole frame must live in ONE GPR bank (epic-cc#6).
///
/// PIC18's bank is `operand()`'s 256-byte `BSR` bank (`addr >> 8`), not an
/// `ram_banks` region: every PIC18 device declares its whole RAM as a single
/// region (`p18f4550` is `[[0x0010, 0x07FF]]`), so a region-based check
/// never fires and a frame crossing `0x100` gets its `MOVLB` emitted in the
/// middle of its own recipe (epic-cc#509). Its snap target is therefore the
/// next `0x100` boundary, not the next region's start, which on a
/// single-region device is the same address the frame just left.
fn routine_base(device: &Device, base: u16, widths: &[u8]) -> u16 {
    let (_, region_end) = device
        .region_for(base)
        .expect("alloc: routine frame base in a device GPR bank");
    let end = frame_end(device, base, widths);
    if device.core == device::Core::Pic18 {
        // Capped at the region end so a partially-mapped bank does not claim
        // the bytes past it. A routine frame is at most 22 bytes, so one
        // step past the current bank always fits.
        let bank_end = (base | 0x00FF).min(region_end);
        if end - 1 <= bank_end {
            return base;
        }
        let next = (base | 0x00FF) + 1;
        let (start, _) = device
            .region_for(next)
            .unwrap_or_else(|| panic!("alloc: routine frame needs a bank past 0x{bank_end:X}"));
        return next.max(start); // skip any unmapped gap between regions
    }
    if end - 1 <= region_end {
        return base; // the whole frame fits in the base's bank
    }
    // The frame would straddle: snap to the next bank's start.
    let next = region_end + 1;
    device
        .region_for(next)
        .map(|(s, _)| s)
        .unwrap_or_else(|| panic!("alloc: routine frame needs a bank past 0x{region_end:X}"))
}

/// `base` unchanged for an ordinary function; `routine_base`-rounded for a
/// runtime routine (float ones included), so its frame stays in one GPR
/// bank. The recipes are genuinely skip-sensitive and banked: the mul/div
/// chains end their loops with real `BNC`/`BRA` branches, but the float,
/// signed-divmod and conversion bodies test a frame byte with `BTFSC`/
/// `BTFSS` and skip the instruction that follows, and that instruction
/// reads a frame byte too. A `MOVLB` between the two changes the operand
/// the skipped instruction names, not just its position, so the whole frame
/// must sit in one bank (epic-cc#357, epic-cc#509). Shared by the
/// main-context and ISR-context base-assignment loops (epic-cc#6).
fn round_if_routine(
    device: &Device,
    f: &str,
    base: u16,
    locals_widths: &HashMap<String, Vec<u8>>,
) -> u16 {
    if !ir::is_runtime_routine(f) {
        return base;
    }
    // Baseline keeps the unrounded base: its 16-byte banks cannot hold a
    // 19-20 byte routine frame whole, and whole-frame single-bank is not
    // the soundness condition here. Each value still places single-bank
    // via place_contiguous, and the recipes' skip chains stay inside one
    // value (unconditional FSR reassertion outside chains), so spanning
    // frames stay sound while wasting no bank tails on a 41-byte device.
    if device.core == device::Core::PicBaseline {
        return base;
    }
    routine_base(device, base, &locals_widths[f])
}

/// The address just past the highest frame byte in `base`, floored at
/// `region_start` so an empty overlay still reports its own start.
fn overlay_end(
    device: &Device,
    base: &HashMap<String, u16>,
    locals_widths: &HashMap<String, Vec<u8>>,
    region_start: u16,
) -> u16 {
    base.iter()
        .map(|(f, &b)| frame_end(device, b, &locals_widths[f]))
        .max()
        .unwrap_or(region_start)
        .max(region_start)
}

/// One function's liveness-overlay frame: the distinct slot widths in
/// allocation order (the order `frame_end` walks) and the frame's byte size
/// (the colored peak, not the width sum). Values whose live ranges never
/// overlap share a slot, so a frame shrinks from the width sum to the peak
/// simultaneous demand. The coloring is deterministic: values are processed
/// in (range start, placement order), each reusing the lowest slot whose
/// interval is disjoint (epic-cc#172).
struct FrameLayout {
    widths: Vec<u8>,
    size: u16,
    /// value name -> slot index into `widths`. Every def and param gets a
    /// slot; the locals placement reads this to put each value at its
    /// slot's address.
    slot_of: HashMap<String, usize>,
    /// Per-slot access heat, parallel to `widths`: the operand reads plus
    /// the defining write of every value the slot holds. `window_order`
    /// reads it to rank the slots of a frame whose access-window prefix is
    /// worth filling (epic-cc#535).
    heat: Vec<u32>,
}

/// Blocks of `f` in liveness and placement order: the entry (the function's
/// first block) is index 0, the rest follow in label order.
///
/// Pinning the entry is load-bearing: opt can leave an unnamed numeric entry
/// next to a block an inline named (`merged.exit`, epic-cc#446), and ranking
/// a name below a number would sort it off index 0, where `frame_layout`
/// reads params and entry live-ins from (epic-cc#448).
fn block_order(f: &ir::Func) -> Vec<&ir::Block> {
    let (entry, rest) = f
        .blocks
        .split_first()
        .expect("alloc: a function has an entry block");
    let mut order: Vec<&ir::Block> = Vec::with_capacity(f.blocks.len());
    order.push(entry);
    order.extend(rest);
    order[1..].sort_by_key(|b| match b.label.parse::<u64>() {
        Ok(v) => (1u8, v),
        Err(_) => (0u8, 0),
    });
    order
}

/// Compute a function's liveness-colored frame from its IR. Each value's
/// live interval is `[min(def, uses, phi pred ends), max(...)]` in linear
/// block order (entry first, then label order). Phi incoming values are
/// used at the END of their predecessor (isel emits the incoming copies
/// there), and a phi destination is live from the earliest predecessor end
/// (its first copy) through its last use. A loop-carried value (use before
/// def in linear order) gets an interval spanning the loop, so it can never
/// alias a value it is co-live with; a dead def is a point interval,
/// immediately reusable. Greedy first-fit coloring reuses the lowest slot
/// whose interval is disjoint; the slot's width grows to the widest
/// occupant.
fn frame_layout(f: &ir::Func, resolved: &PtrResolution, va_size: u16) -> FrameLayout {
    let order = block_order(f);
    let idx: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.as_str(), i))
        .collect();
    let block_len: Vec<u16> = order.iter().map(|b| b.insts.len() as u16).collect();

    // defs: value name -> (block index, def position, width, placement
    // order, memory object). Params are defined at entry and come first;
    // defined values follow in instruction order. The placement order
    // breaks ties in the coloring, so same-start values keep the documented
    // params-first, instruction-order layout. A memory object (an alloca or
    // a byval/sret param) is a RAM region the code reads and writes through
    // derived pointers, not a value with a def-use range: its slot must stay
    // reserved for the whole function, so it gets a full-function interval.
    let mut defs: HashMap<String, (usize, u16, u8, usize, bool)> = HashMap::new();
    let mut order_idx = 0usize;
    for p in &f.params {
        defs.insert(
            p.name.clone(),
            (0, 0, p.width, order_idx, p.byval.is_some() || p.sret),
        );
        order_idx += 1;
    }
    // The va region: the contiguous frame bytes a variadic callee's extra
    // args land in, one region per variadic function, sized to the widest
    // call site. Full-function interval like any memory object: the
    // callee's `va_arg` reads are live across the whole body and the
    // caller's arg copies happen at each call point.
    if va_size > 0 {
        defs.insert("__va".to_string(), (0, 0, va_size as u8, order_idx, true));
        order_idx += 1;
    }
    for (i, b) in order.iter().enumerate() {
        for (pos, inst) in b.insts.iter().enumerate() {
            if let Some((name, width)) = def_width(inst, resolved, &f.name) {
                let mem = matches!(inst, ir::Inst::Alloca(_));
                defs.insert(name, (i, pos as u16, width, order_idx, mem));
                order_idx += 1;
            }
        }
    }

    // uses: value name -> set of (block, position). A phi's incoming values
    // are used at the END of their predecessor (isel emits the incoming
    // copies there), not in the merge block. Every operand is recorded,
    // including GEP dsts (which define no slot but whose uses drive the
    // operand propagation below); the interval loop filters to defs.
    let mut uses: HashMap<String, HashSet<(usize, u16)>> = HashMap::new();
    for (i, b) in order.iter().enumerate() {
        for (pos, inst) in b.insts.iter().enumerate() {
            if let ir::Inst::Phi(p) = inst {
                for (v, pred) in &p.incoming {
                    let vn = ir::val_name(v);
                    let pi = idx[pred.as_str()];
                    uses.entry(vn).or_default().insert((pi, block_len[pi]));
                }
                continue;
            }
            for v in ir::read_vals(inst) {
                uses.entry(v).or_default().insert((i, pos as u16));
            }
            if let ir::Inst::Gep(g) = inst {
                uses.entry(g.dst.clone())
                    .or_default()
                    .insert((i, pos as u16));
            }
        }
    }

    // A GEP's base and term regs are re-read by isel at every load/store
    // through the GEP's result pointer (the FSR setup recomputes the
    // address from the index each time), so their liveness extends to the
    // last use of the GEP dst: propagate the dst's uses onto its operands.
    let mut gep_operands: HashMap<String, Vec<String>> = HashMap::new();
    for b in &order {
        for inst in &b.insts {
            if let ir::Inst::Gep(g) = inst {
                let mut ops = Vec::new();
                if let ir::GepBase::Reg(r) = &g.base {
                    ops.push(r.clone());
                }
                ops.extend(g.terms.iter().map(|(_, r)| r.clone()));
                gep_operands.insert(g.dst.clone(), ops);
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (dst, ops) in &gep_operands {
            let dst_uses: Vec<(usize, u16)> = uses
                .get(dst)
                .map(|u| u.iter().copied().collect())
                .unwrap_or_default();
            for op in ops {
                if let Some(op_uses) = uses.get_mut(op) {
                    let before = op_uses.len();
                    op_uses.extend(dst_uses.iter().copied());
                    if op_uses.len() != before {
                        changed = true;
                    }
                }
            }
        }
    }

    // Loop-aware liveness: a value used in a loop body is live across the
    // back-edge, so its slot is occupied at the body's end and the header's
    // start even when its last linear use is earlier. A backward fixpoint
    // computes live-in/live-out per block; the interval loop then extends
    // each value's range to the start of every block it is live-in to and
    // the end of every block it is live-out of. Phi incomings are excluded
    // from the use sets (they are used precisely at the pred end) and phi
    // dsts from the def sets (they are written by the pred-end copies).
    let norm = |t: &str| {
        t.strip_prefix("label ")
            .unwrap_or(t)
            .trim_start_matches('%')
            .to_string()
    };
    let mut succ: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, b) in order.iter().enumerate() {
        let mut ss = Vec::new();
        for inst in &b.insts {
            match inst {
                ir::Inst::Br(br) => ss.push(idx[&norm(&br.target)[..]]),
                ir::Inst::BrCond(bc) => {
                    ss.push(idx[&norm(&bc.t)[..]]);
                    ss.push(idx[&norm(&bc.f)[..]]);
                }
                ir::Inst::Switch(sw) => {
                    ss.push(idx[&norm(&sw.default)[..]]);
                    for (_, l) in &sw.cases {
                        ss.push(idx[&norm(l)[..]]);
                    }
                }
                _ => {}
            }
        }
        succ.insert(i, ss);
    }
    // Use-before-def per block: a value used in a block before (or at) its
    // own def there is live at the block's start; a value used only after
    // its def is not. Phi dsts are defined at the block start, so their uses
    // never count as use-before-def; phi incomings are excluded from `uses`
    // (they are live precisely at the pred end, via phi_pred_ends).
    let mut use_before_def: Vec<HashSet<String>> = vec![HashSet::new(); order.len()];
    for (v, us) in &uses {
        for &(b, p) in us {
            let def_pos = defs.get(v).map(|&(d, pd, _, _, _)| (d, pd));
            let before = match def_pos {
                Some((d, pd)) => d != b || p <= pd,
                None => true,
            };
            if before {
                use_before_def[b].insert(v.clone());
            }
        }
    }
    let mut def_set: Vec<HashSet<String>> = vec![HashSet::new(); order.len()];
    for (v, &(d, _, _, _, mem)) in &defs {
        if !mem {
            def_set[d].insert(v.clone());
        }
    }
    let mut live_in: Vec<HashSet<String>> = vec![HashSet::new(); order.len()];
    let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); order.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for i in (0..order.len()).rev() {
            let mut lo_new: HashSet<String> = HashSet::new();
            for s in &succ[&i] {
                lo_new.extend(live_in[*s].iter().cloned());
            }
            if lo_new != live_out[i] {
                live_out[i] = lo_new;
                changed = true;
            }
            let mut li_new: HashSet<String> = use_before_def[i].clone();
            for v in live_out[i].iter() {
                if !def_set[i].contains(v) {
                    li_new.insert(v.clone());
                }
            }
            if li_new != live_in[i] {
                live_in[i] = li_new;
                changed = true;
            }
        }
    }
    // Predecessor map for the interval extension below.
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in &succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }

    // phi destinations: the ends of their merge block's predecessors (the
    // copy points where isel writes the slot).
    let mut phi_pred_ends: HashMap<String, HashSet<(usize, u16)>> = HashMap::new();
    for b in &order {
        for inst in &b.insts {
            if let ir::Inst::Phi(p) = inst {
                for (_, pred) in &p.incoming {
                    let pi = idx[pred.as_str()];
                    phi_pred_ends
                        .entry(p.dst.clone())
                        .or_default()
                        .insert((pi, block_len[pi]));
                }
            }
        }
    }

    // Live interval per value: [min(def, uses, pred ends), max(...)] in
    // linear order. A loop-carried value (use before def in linear order)
    // spans the loop, so it cannot alias a co-live value; a dead def is a
    // point interval, immediately reusable. A memory object spans the whole
    // function (its slot is a RAM region, live from entry to exit), so it
    // never aliases anything.
    let mut vals: Vec<(&String, (usize, u16), (usize, u16), u8, usize)> = Vec::new();
    for (v, &(d, p_d, w, o, mem)) in &defs {
        let (lo, hi) = if mem {
            (
                (0usize, 0u16),
                (
                    order.len().saturating_sub(1),
                    block_len.last().copied().unwrap_or(0),
                ),
            )
        } else {
            let mut lo = (d, p_d);
            let mut hi = (d, p_d);
            if let Some(us) = uses.get(v) {
                for &(b, p) in us {
                    lo = lo.min((b, p));
                    hi = hi.max((b, p));
                }
            }
            if let Some(ps) = phi_pred_ends.get(v) {
                for &(b, p) in ps {
                    lo = lo.min((b, p));
                    hi = hi.max((b, p));
                }
            }
            // Loop-aware extension: a value live-in to a block (live across
            // a back-edge into it) occupies its slot from that block's start
            // through the end of every predecessor (the value is live at the
            // pred ends too). The entry block's live-ins are params, already
            // covered by their defs.
            if let Some(ps) = preds.get(&0) {
                for &bi in ps {
                    if live_in[0].contains(v) {
                        hi = hi.max((bi, block_len[bi]));
                    }
                }
            }
            for (bi, li) in live_in.iter().enumerate().skip(1) {
                if li.contains(v) {
                    lo = lo.min((bi, 0));
                    if let Some(ps) = preds.get(&bi) {
                        for &pi in ps {
                            hi = hi.max((pi, block_len[pi]));
                        }
                    }
                }
            }
            (lo, hi)
        };
        vals.push((v, lo, hi, w, o));
    }
    vals.sort_by(|a, b| a.1.cmp(&b.1).then(a.4.cmp(&b.4)));

    // Greedy first-fit coloring: reuse the lowest slot whose interval is
    // disjoint from the new value's; the slot's width grows to the widest
    // occupant.
    let mut slots: Vec<((usize, u16), (usize, u16), u8)> = Vec::new();
    let mut slot_of: HashMap<String, usize> = HashMap::new();
    for (v, lo, hi, w, _) in &vals {
        let mut placed = None;
        for (i, (slo, shi, _)) in slots.iter().enumerate() {
            if hi < slo || shi < lo {
                placed = Some(i);
                break;
            }
        }
        match placed {
            Some(i) => {
                slots[i].0 = slots[i].0.min(*lo);
                slots[i].1 = slots[i].1.max(*hi);
                slots[i].2 = slots[i].2.max(*w);
                slot_of.insert((*v).clone(), i);
            }
            None => {
                slots.push((*lo, *hi, *w));
                slot_of.insert((*v).clone(), slots.len() - 1);
            }
        }
    }
    let widths: Vec<u8> = slots.iter().map(|&(_, _, w)| w).collect();
    // Per-slot access heat: the operand reads plus the defining write of
    // every value the slot holds. A memory object (alloca, byval/sret param,
    // va region) is touched through derived pointers, so count one so it
    // still ranks, but never above a real value.
    //
    // The `widths` order stays as the coloring left it: it feeds
    // `frame_end`, which on a split-bank device can move a callee's base.
    let mut heat: Vec<u32> = vec![0; widths.len()];
    for (v, &(_, _, _, _, mem)) in &defs {
        let slot = slot_of[v];
        let reads = if mem {
            0
        } else {
            uses.get(v).map_or(0, |u| u.len() as u32)
        };
        heat[slot] += reads + 1;
    }
    let size: u16 = widths.iter().map(|&w| u16::from(w)).sum();
    FrameLayout {
        widths,
        size,
        slot_of,
        heat,
    }
}

/// `fl.widths` reordered by the permutation `perm` (`perm[new] = old`), the
/// width vector the placed order would produce.
fn perm_widths(fl: &FrameLayout, perm: &[usize]) -> Vec<u8> {
    perm.iter().map(|&old| fl.widths[old]).collect()
}

/// The slot permutation for a frame placed at `base`, or `None` to leave the
/// coloring's own order alone.
///
/// `win_end` is the exclusive end of the device's access window (`0x60` on
/// PIC18, whose window is `0x000-0x05F`). `operand()` reaches an address in
/// that window with no `MOVLB`; the frame overlay sits at `gpr_start`, so a
/// frame's low bytes are the bank-free ones and a frame that ends past the
/// window spends `MOVLB` on its tail. Permuting the slots by heat puts the
/// hottest bytes in that prefix.
///
/// Two guards keep this from hurting anything:
///
/// - Only a frame that STRADDLES the window's end can gain: a frame wholly
///   inside it is already bank-free byte for byte, and one wholly outside it
///   is banked throughout, so in both cases reordering changes nothing but
///   the `MOVFF` copy runs it can break (a run of adjacent slots a memcpy
///   walks as one looped move; splitting it costs one instruction per byte,
///   epic-cc#535's measured 3-word `math` regression).
/// - The permutation is stable, so equal heat keeps the coloring's order and
///   a frame whose accesses do not distinguish its slots lays out exactly as
///   before.
///
/// Returns `perm` with `perm[new_position] = old_slot`.
fn window_order(base: u16, win_end: u16, fl: &FrameLayout) -> Option<Vec<usize>> {
    let frame_end = u32::from(base) + u32::from(fl.size);
    if base >= win_end || frame_end <= u32::from(win_end) {
        return None;
    }
    let mut perm: Vec<usize> = (0..fl.widths.len()).collect();
    perm.sort_by_key(|&i| std::cmp::Reverse(fl.heat[i]));
    Some(perm)
}

/// Every function transitively reachable from `roots` over the caller ->
/// callee map `edges` (the roots included). A visited set keeps a call cycle
/// (rejected earlier by the topological sort) from looping forever.
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

/// Largest-first bin-pack of the floating globals into per-bank cursors
/// (the fallback the sequential path used when it could not fit them all).
/// Independent per-bank cursors let a later small global use a bank the
/// sequential cursor already passed. Returns `None` when no arrangement
/// fits.
fn bin_pack(
    device: &Device,
    fixed: &[(String, u16, u16)],
    floating: &[&ir::Global],
    start: u16,
) -> Option<HashMap<String, u16>> {
    let mut cursors: Vec<BankCursor> = device
        .ram_banks
        .iter()
        .map(|&(s, e)| BankCursor {
            end: e,
            next_free: s.max(start),
        })
        .collect();
    for c in &mut cursors {
        for (_, fa, fs) in fixed {
            if c.next_free >= *fa && c.next_free < *fa + *fs {
                c.next_free = *fa + *fs;
            }
        }
    }
    let mut order: Vec<&ir::Global> = floating.to_vec();
    order.sort_by(|a, b| b.size.cmp(&a.size));
    let mut out = HashMap::new();
    for g in order {
        let width = g.size;
        let align = width.min(2);
        let mut placed = None;
        for cursor in cursors.iter_mut() {
            let mut base = cursor.next_free;
            if base % u16::from(align) != 0 {
                base += u16::from(align) - (base % u16::from(align));
            }
            let mut bumped = true;
            while bumped {
                bumped = false;
                for (_, fa, fs) in fixed {
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
}

/// The end address of a global arrangement: the highest `addr + size`.
fn global_end(map: &HashMap<String, u16>, floating: &[&ir::Global]) -> u16 {
    floating
        .iter()
        .filter_map(|g| map.get(&g.name).map(|a| a + u16::from(g.size)))
        .max()
        .unwrap_or(0)
}

/// Assign every address: globals sequential (even-aligned i16, stepping
/// through banks), locals per the overlay algorithm over the call graph parsed
/// from `edges_text` (`edge <caller> <callee>` lines, order-agnostic; `depth`
/// lines are informational). Panics on a cyclic or unknown-function call
/// graph, and when total demand exceeds the device's GPR space.

/// The byte size of each variadic callee's va region: the maximum over its
/// call sites of the sum of extra-arg widths (the args past the named
/// params, which land in the callee's `__va` region in order).
fn va_sizes(m: &Module) -> HashMap<String, u16> {
    let mut sizes: HashMap<String, u16> = HashMap::new();
    let mut params_len: HashMap<String, usize> = m
        .funcs
        .iter()
        .map(|f| (f.name.clone(), f.params.len()))
        .collect();
    for f in &m.funcs {
        if !f.variadic {
            continue;
        }
        let named = f.params.len();
        let max_w: u16 = 0;
        let _ = max_w;
        sizes.entry(f.name.clone()).or_insert(0);
        params_len.insert(f.name.clone(), named);
    }
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let ir::Inst::Call(c) = inst {
                    if !c.callees.is_empty() {
                        continue; // indirect calls never carry extras today
                    }
                    let Some(&named) = params_len.get(&c.func) else {
                        continue;
                    };
                    if c.args.len() <= named {
                        continue;
                    }
                    let extra: u16 = c.args[named..]
                        .iter()
                        .map(|a| a.ty.map(|t| u16::from(t.bytes())).unwrap_or(2))
                        .sum();
                    let e = sizes.entry(c.func.clone()).or_insert(0);
                    *e = (*e).max(extra);
                }
            }
        }
    }
    sizes
}

pub fn allocate(device: &Device, m: &Module, edges_text: &str) -> AllocLayout {
    // The va region size frame_layout reserves: the widest call site, with
    // a one-byte floor so a variadic function whose call sites pass no
    // extra args still has a base address for its va_start (epic-cc#391).
    fn floored_va_size(f: &ir::Func, va_sizes: &HashMap<String, u16>) -> u16 {
        let vs = va_sizes.get(&f.name).copied().unwrap_or(0);
        if f.variadic {
            vs.max(1)
        } else {
            vs
        }
    }
    // iselcore's pointer resolution: a pointer select seeded as an indirect
    // slot materializes its two address bytes into the dst slot, so the
    // dst needs a RAM slot; a folded select is virtual and defines none.
    let resolved = resolve_pointers(m);

    // Steps 1-5 shape the frame overlay over a region whose start is still
    // open. Nothing here depends on where the globals land, which is what
    // lets PIC18 place the overlay BELOW them (step 5's caller).

    // 1. locals_widths(f) = the liveness-overlay slot widths of f's params
    // and defined values, in allocation order (the order `frame_end` walks
    // and the locals placement reproduces). Values whose live ranges never
    // overlap share a slot, so a frame shrinks from the width sum to the
    // peak simultaneous demand. locals_size(f) is the colored frame's byte
    // size (epic-cc#172).
    let mut locals_widths: HashMap<String, Vec<u8>> = HashMap::new();
    let mut locals_size: HashMap<String, u16> = HashMap::new();
    let va_sizes = va_sizes(m);
    for f in &m.funcs {
        let fl = frame_layout(f, &resolved, floored_va_size(f, &va_sizes));
        locals_widths.insert(f.name.clone(), fl.widths);
        locals_size.insert(f.name.clone(), fl.size);
    }

    // 2. Call graph from the edge text.
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

    // 3. Topological order (recursion is rejected by callgraph; panics
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

    // 4. depth_end(f) = locals_size(f) + max(0, max over callees depth_end(c)).
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

    let mut callers: HashMap<String, Vec<String>> = HashMap::new();
    for (p, cs) in &edges {
        for c in cs {
            callers.entry(c.clone()).or_default().push(p.clone());
        }
    }
    let isr_names: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr)
        .map(|f| f.name.as_str())
        .collect();

    // Steps 5 and 5b as one operation over a frame-region start, so the
    // region can be derived before the globals (PIC18, below) or after
    // them (PIC14) without the derivation existing twice.
    let assign_bases =
        |region_start: u16| -> (HashMap<String, u16>, Option<u16>, Option<u16>, Option<u16>) {
            // 5. base(f) = max over direct callers of the caller's PHYSICAL
            // frame end; roots at region_start, in forward topo order so every
            // caller precedes its callees.

            // The virtual sum base(p) + locals_size[p] is NOT used: a caller
            // whose frame spills past a bank region end ends beyond that sum,
            // and a callee based on it could land in the gap at the next
            // region's start, exactly where the caller's spill locals live
            // while both frames are live. frame_end walks the caller's actual
            // widths through place_contiguous, so it matches step 7's
            // placement exactly, hole bytes included.
            let mut base: HashMap<String, u16> = HashMap::new();
            for f in &topo {
                let b = match callers.get(f) {
                    Some(ps) => ps
                        .iter()
                        .map(|p| frame_end(device, base[p], &locals_widths[p]))
                        .max()
                        .expect("alloc: empty caller list"),
                    None => region_start,
                };
                // A runtime routine's frame must stay inside ONE GPR bank: its
                // skip-sensitive recipe loops cannot tolerate a BANKSEL between a
                // test and its target, or inside a carry idiom. The base is rounded
                // when the derived frame would straddle a bank boundary; float
                // routines round exactly like integer ones (epic-cc#357).
                let b = round_if_routine(device, f, b, &locals_widths);
                base.insert(f.clone(), b);
            }

            // 5b. The disjoint ISR region: an ISR root's frame base is AFTER
            // the main context's total (the max physical frame end over the
            // NON-ISR roots' contexts), not the region start. The ISR can
            // preempt main at any point, so a preempted main's live frames must
            // never overlap the ISR context's. The physical end equals the
            // depth sum when no local crosses a bank gap and is strictly larger
            // (hence safer) when an i16 at a region tail leaves a hole.

            // The loop above is exact for every non-ISR context (a main-side
            // function can reach an ISR-context copy only through a dispatch
            // on a shared-storage callback address, whose frame the ISR
            // region then absorbs via the max over the reachable sets), so
            // the disjoint base derives from its results; each ISR context is
            // then re-derived from that base in topo order.

            // Set inside the ISR-region block below: `Some` only in priority mode.
            let mut isr_low_save: Option<u16> = None;
            // The compat/high ISR's area for the PROD/FSR1 bytes epic-cc#477
            // adds: the fixed block has no room, so it is carved here.
            let mut isr_save: Option<u16> = None;
            let mut isr_hi_save: Option<u16> = None;
            if !isr_names.is_empty() {
                let isr_roots: Vec<&String> = topo
                    .iter()
                    .filter(|f| !callers.contains_key(*f) && isr_names.contains(f.as_str()))
                    .collect();
                // Priority-partitioned ISR roots: the high ISR can preempt the
                // low one (and main) mid-call, so each priority's context needs
                // its own disjoint overlay region. Compatibility mode has no
                // high roots and behaves exactly as before.
                let hi_roots: Vec<&&String> = isr_roots
                    .iter()
                    .filter(|f| {
                        m.funcs
                            .iter()
                            .find(|g| g.name.as_str() == f.as_str())
                            .is_some_and(|g| g.irq_priority == 1)
                    })
                    .collect();
                let lo_roots: Vec<&&String> = isr_roots
                    .iter()
                    .filter(|f| !hi_roots.iter().any(|h| h.as_str() == f.as_str()))
                    .collect();
                let non_isr_roots: Vec<&String> = topo
                    .iter()
                    .filter(|f| !callers.contains_key(*f) && !isr_names.contains(f.as_str()))
                    .collect();
                // The disjoint base = max physical frame end over the non-ISR roots'
                // contexts (the main context's total). No non-ISR root: the ISR is
                // the only root, so it simply starts at region_start.
                let isr_base = non_isr_roots
                    .iter()
                    .flat_map(|r| reachable(&[r.as_str()], &edges))
                    .map(|f| frame_end(device, base[&f], &locals_widths[&f]))
                    .max()
                    .unwrap_or(region_start);
                // Re-derive each priority's context from its disjoint base in topo
                // order (callers precede callees, and every caller of a context
                // function is itself in that context after the legalize
                // duplication).
                let assign_region =
                    |base: &mut HashMap<String, u16>, roots: &[&&String], region_base: u16| {
                        let ctx: HashSet<String> = roots
                            .iter()
                            .flat_map(|r| reachable(&[r.as_str()], &edges))
                            .collect();
                        for f in &topo {
                            if !ctx.contains(f) {
                                continue;
                            }
                            let b = if roots.iter().any(|r| r.as_str() == f.as_str()) {
                                region_base
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
                    };
                // Priority mode (both priorities present): the low ISR's 19-byte
                // context-save area sits at the low region's base, below the low
                // frames, so it is disjoint from every context by construction
                // (the access-window argument shows no float frame can land
                // inside it). Compatibility mode keeps the historical layout
                // byte-identical: no shift, no save area.
                let priority_mode = !lo_roots.is_empty() && !hi_roots.is_empty();
                // Every ISR context needs these save bytes, whichever
                // priority it runs at, so each region's base gets its own
                // carved area of the same size (epic-cc#477, epic-cc#532,
                // epic-cc#641: FSR1, PROD, TABLAT, PCLATH, PCLATU).
                // PIC14/PIC14E have no MULWF/PROD, no FSR1 copy loop and no
                // TBLRD, so their layout stays byte-identical.
                let needs_prod_save = device.core == device::Core::Pic18;
                const ISR_SAVE_BYTES: u16 = 7;
                const LOW_SAVE_BYTES: u16 = 19;
                let lo_base = if priority_mode {
                    isr_low_save = Some(isr_base);
                    isr_base + LOW_SAVE_BYTES
                } else if needs_prod_save {
                    // Compatibility mode: the lone handler's own save area,
                    // carved above the ISR context base so it is disjoint from
                    // main's frames by the same construction the priority path
                    // uses.
                    isr_save = Some(isr_base);
                    isr_base + ISR_SAVE_BYTES
                } else {
                    isr_base
                };
                assign_region(&mut base, &lo_roots, lo_base);
                // The high region sits above everything the high ISR can preempt
                // (main and low frames): max frame end over all assigned bases.
                let hi_base = base
                    .iter()
                    .map(|(f, b)| frame_end(device, *b, &locals_widths[f]))
                    .max()
                    .unwrap_or(isr_base);
                let hi_base = if priority_mode && needs_prod_save {
                    // The high handler needs the same seven bytes; carve them
                    // from its own region so the two ISRs' areas stay disjoint.
                    isr_hi_save = Some(hi_base);
                    hi_base + ISR_SAVE_BYTES
                } else {
                    hi_base
                };
                assign_region(&mut base, &hi_roots, hi_base);
            }
            (base, isr_low_save, isr_save, isr_hi_save)
        };

    // PIC18 puts the frame overlay BELOW the globals. The access bank
    // (0x000-0x05F) is the only RAM `isel-pic18`'s `operand()` reaches with
    // no `MOVLB`, and frames are what its direct file-register operands
    // name: globals mostly move through `MOVFF` and `FSR`, which carry a
    // full 12-bit address and never touch `BSR`. Globals first therefore
    // spends the window on the operands that do not need it.

    // Swapping the two blocks moves no byte of demand, only its order, so
    // the total holds up to each block's own alignment and any pinned
    // global. PIC14 has no access bank (common RAM, its analogue, is
    // carved out of every bank already) and keeps the historical order.
    let placed_frames = device
        .access_bank
        .is_some()
        .then(|| assign_bases(device.gpr_start()));
    // A global pinned by address (`__at`) inside the overlay's span has
    // nowhere to go, so those modules keep the globals-first layout, where
    // the frames start above every pinned address. The scan is over every
    // pinned global, const included: whether a const ends up in RAM is
    // decided below, and falling back is the safe way to be wrong.
    let placed_frames = placed_frames.filter(|(base, _, _, _)| {
        let top = overlay_end(device, base, &locals_widths, device.gpr_start());
        !m.globals
            .iter()
            .any(|g| matches!(g.addr, Some(a) if a < top))
    });
    let global_start = match &placed_frames {
        Some((b, _, _, _)) => overlay_end(device, b, &locals_widths, device.gpr_start()),
        None => device.gpr_start(),
    };

    // 6. Globals: sequential, aligned to at most two bytes (i16 -> even
    // address; larger arrays advance sequentially), stepping through the banks as bank 0 GPR fills up. Each global spans
    // `size` bytes (an `[N x T]` array takes N addresses, not one), so a
    // sized array advances the free pointer by its byte count. Const globals
    // get no RAM address (their bytes live in flash) but are still recorded
    // so the map text can list them. A const that is used as a plain pointer
    // call argument (direct `@.str`/`@k` or a `getelementptr` over it) is
    // instead placed in RAM as a static initialized copy so the callee's
    // generic pointer load (FSR/INDF through the param slot) reads the
    // literal correctly. Only small consts (<=255 bytes) are copied; larger
    // tables stay in flash.
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut fixed: Vec<(String, u16, u16)> = Vec::new(); // (name, addr, size)
    let mut floating: Vec<&ir::Global> = Vec::new();
    let mut const_to_ram: HashSet<String> = HashSet::new();
    {
        use ir::GepBase;
        let mut func_gep: HashMap<String, HashMap<String, GepBase>> = HashMap::new();
        for f in &m.funcs {
            let mut m2: HashMap<String, GepBase> = HashMap::new();
            for b in &f.blocks {
                for inst in &b.insts {
                    if let ir::Inst::Gep(g) = inst {
                        m2.insert(g.dst.clone(), g.base.clone());
                    }
                }
            }
            func_gep.insert(f.name.clone(), m2);
        }
        for f in &m.funcs {
            let gep_map = func_gep.get(&f.name).unwrap();
            let find_const_base = |reg: &str| -> Option<String> {
                let mut cur = reg.to_string();
                let mut seen: HashSet<String> = HashSet::new();
                loop {
                    if !seen.insert(cur.clone()) {
                        break;
                    }
                    match gep_map.get(&cur) {
                        Some(GepBase::Global(name)) => {
                            if m.globals.iter().any(|gl| gl.name == *name && gl.is_const) {
                                return Some(name.clone());
                            } else {
                                return None;
                            }
                        }
                        Some(GepBase::Reg(r)) => cur = r.clone(),
                        None => return None,
                    }
                }
                None
            };
            for b in &f.blocks {
                for inst in &b.insts {
                    if let ir::Inst::Call(c) = inst {
                        for arg in &c.args {
                            if arg.ty.is_none() {
                                match &arg.val {
                                    ir::Val::Global(g) => {
                                        if m.globals.iter().any(|gl| &gl.name == g && gl.is_const) {
                                            if let Some(gl) =
                                                m.globals.iter().find(|gl| &gl.name == g)
                                            {
                                                if gl.size <= 255 {
                                                    const_to_ram.insert(g.clone());
                                                }
                                            }
                                        }
                                    }
                                    ir::Val::Reg(r) => {
                                        if let Some(base) = find_const_base(r) {
                                            if let Some(gl) =
                                                m.globals.iter().find(|gl| gl.name == base)
                                            {
                                                if gl.size <= 255 {
                                                    const_to_ram.insert(base);
                                                }
                                            }
                                        }
                                    }
                                    ir::Val::Const(_) => {}
                                }
                            }
                        }
                    }
                    // A pointer select whose arms are const globals is a runtime
                    // address VALUE when the arms do not fold to a common base
                    // (iselcore seeds it as an indirect slot): the selected
                    // arm's bytes are read through the slot with RAM semantics,
                    // so each const arm must be copied to RAM. A select that
                    // folds (same base, e.g. the ccp_sel shape) keeps its
                    // const in flash: loads lower via the fold's RETLW/TBLRD
                    // path (epic-cc#147).
                    if let ir::Inst::Select(s) = inst {
                        if !s.ptr {
                            continue;
                        }
                        let const_base = |v: &ir::Val| -> Option<String> {
                            match v {
                                ir::Val::Global(g) => {
                                    if m.globals.iter().any(|gl| &gl.name == g && gl.is_const) {
                                        Some(g.clone())
                                    } else {
                                        None
                                    }
                                }
                                ir::Val::Reg(r) => find_const_base(r),
                                ir::Val::Const(_) => None,
                            }
                        };
                        // `ba != bb` is the fold test: equal const bases fold
                        // (ccp_sel shape, const stays in flash); different
                        // bases, or a const arm against a RAM/runtime arm,
                        // seed the select and need each const arm in RAM.
                        let (ba, bb) = (const_base(&s.a), const_base(&s.b));
                        if ba != bb {
                            for g in [ba, bb].into_iter().flatten() {
                                if let Some(gl) = m.globals.iter().find(|gl| gl.name == g) {
                                    if gl.size <= 255 {
                                        const_to_ram.insert(g);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // RAM globals have no 255-byte ceiling: `Global.size` is `u16` and
    // placement below packs (and, on PIC14E, straddles) them like any
    // other width. A global nothing fits still fails precisely at the
    // `no arrangement fits` panic below, never silently.
    for g in &m.globals {
        if g.is_const && !const_to_ram.contains(&g.name) {
            const_globals.insert(g.name.clone());
        } else if let Some(a) = g.addr {
            fixed.push((g.name.clone(), a, g.size));
        } else {
            floating.push(g);
        }
    }
    // Floating globals are placed with the same sequential -> bin-pack
    // strategy as before, but pinned addresses are respected and the
    // sequential cursor is bumped past any overlap.
    let mut globals: HashMap<String, u16> = HashMap::new();
    for (name, addr, _) in &fixed {
        globals.insert(name.clone(), *addr);
    }
    let (floating_map, frames_fit): (HashMap<String, u16>, bool) = if floating.is_empty() {
        (HashMap::new(), true)
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
                let width = g.size;
                // If the next placement would overlap a fixed region, bump
                // past it before asking try_place_at.
                let mut candidate = addr;
                loop {
                    let mut bumped = false;
                    for (_, fa, fs) in &fixed {
                        if candidate < *fa + *fs
                            && candidate + u16::from(width) > *fa
                            && candidate >= *fa
                            && candidate < *fa + *fs
                        {
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
                addr = physical_end(device, start, u16::from(width));
                // Bump addr past any fixed that it now sits inside.
                for (_, fa, fs) in &fixed {
                    if addr > *fa && addr <= *fa + *fs {
                        addr = *fa + *fs;
                    }
                }
            }
            Some(out)
        };
        // Prefer the arrangement with the lower footprint. The sequential
        // order preserves the .ll order, but a large global after small
        // ones wastes the tail of the first bank (epic-taskmgr's 80-byte
        // task table lands in bank 1 on the 877A, pushing the frames past
        // the last bank, epic-hal#86). The largest-first bin-pack closes
        // that gap; when both fit, the tighter end wins and the layout
        // stays otherwise unchanged (small fixtures place identically).
        let best_at = |start: u16| -> Option<HashMap<String, u16>> {
            let seq = {
                let mut start = start;
                for (_, fa, fs) in &fixed {
                    if start >= *fa && start < *fa + *fs {
                        start = *fa + *fs;
                    }
                }
                try_float(start)
            };
            let bin = bin_pack(device, &fixed, &floating, start);
            match (&seq, &bin) {
                (Some(s), Some(b)) if global_end(b, &floating) < global_end(s, &floating) => {
                    Some(b.clone())
                }
                (Some(s), _) => Some(s.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            }
        };
        let no_fit = |start: u16| -> String {
            let demand: u32 = floating.iter().map(|g| u32::from(g.size)).sum::<u32>()
                + fixed.iter().map(|(_, _, s)| u32::from(*s)).sum::<u32>();
            let capacity: u32 = device
                .ram_banks
                .iter()
                .map(|&(s, e)| u32::from(e.max(start).max(s)) - u32::from(s.max(start)) + 1)
                .sum();
            format!(
                "alloc: no arrangement of {} global(s) fits {}'s {} GPR bank window(s) from \
                 0x{start:X} up (total demand {demand} bytes, capacity above that start \
                 {capacity} bytes, neither sequential nor largest-first bin-packing fits)",
                floating.len() + fixed.len(),
                device.name,
                device.ram_banks.len(),
            )
        };
        // The frames-first order hands the globals a shorter window than
        // the historical one, and the two pack differently around region
        // holes and pinned addresses. A density win is never worth failing
        // to compile, so a module whose globals no longer fit above the
        // overlay goes back to globals-first rather than panicking.
        match best_at(global_start) {
            Some(map) => (map, true),
            None if global_start != device.gpr_start() => (
                best_at(device.gpr_start())
                    .unwrap_or_else(|| panic!("{}", no_fit(device.gpr_start()))),
                false,
            ),
            None => panic!("{}", no_fit(global_start)),
        }
    };
    // Re-bind both when the fallback fired: the frames must follow the
    // globals again, exactly as they do on a core with no access bank.
    let placed_frames = if frames_fit { placed_frames } else { None };
    let global_start = if frames_fit {
        global_start
    } else {
        device.gpr_start()
    };
    globals.extend(floating_map);

    // end_of_globals = max over the address map of the physical end (addr +
    // width, or the bank-straddling physical end on PIC14E), floored at the
    // device's GPR start (mirrors isel's layout computation). The
    // scratch/retval bytes live in the device's fixed common RAM, so the
    // first frame base follows the globals directly.
    let end_of_globals =
        m.globals
            .iter()
            .fold(device.gpr_start(), |end, g| match globals.get(&g.name) {
                Some(&a) => end.max(physical_end(device, a, u16::from(g.size))),
                None => end,
            });

    // The overlay's start: the GPR start when the frames were placed ahead
    // of the globals, the first byte past them otherwise.
    let bank0_start = match &placed_frames {
        Some(_) => device.gpr_start(),
        None => end_of_globals.max(global_start),
    };
    let (base, isr_low_save, isr_save, isr_hi_save) = match placed_frames {
        Some(r) => r,
        None => assign_bases(bank0_start),
    };

    // 7. Local addresses: each slot of the liveness-colored frame at the
    // next free frame byte, stepping through the banks via
    // `place_contiguous`/`region_for` (which panics if a frame exceeds the
    // device's last GPR bank, 0x1EF on PIC16F877A), then every value at its
    // slot's address. The slot walk is exactly `frame_end`'s, so the placed
    // end equals the physical end the callee bases were derived from.
    let mut locals: HashMap<String, u16> = HashMap::new();
    let mut local_width: HashMap<String, u8> = HashMap::new();
    // The exclusive end of the device's access window, when it has one: the
    // last bank-free byte is `win_end - 1` (epic-cc#535).
    let win_end = device.access_bank.map(|(_, hi)| hi + 1);
    for f in &m.funcs {
        let b = base[&f.name];
        let fl = frame_layout(f, &resolved, floored_va_size(f, &va_sizes));
        // The frame end the coloring's own slot order produces; every callee
        // base is derived from it (`frame_end` over `locals_widths`), so a
        // permutation that moved it would silently rebase the callees.
        let size_end = frame_end(device, b, &fl.widths);
        // Slot order: heat-first when this frame straddles the access
        // window's end, the coloring's order otherwise. A permutation keeps
        // the frame's byte size but can in principle move the end where a
        // slot crosses a region boundary (p18f2450, the one PIC18 with a
        // split `ram_banks`); fall back rather than rebase the callees.
        let order: Vec<usize> = match win_end.and_then(|we| window_order(b, we, &fl)) {
            Some(p) if frame_end(device, b, &perm_widths(&fl, &p)) == size_end => p,
            _ => (0..fl.widths.len()).collect(),
        };
        let mut slot_addr: Vec<u16> = vec![0; fl.widths.len()];
        let mut addr = b;
        for &slot in &order {
            let w = fl.widths[slot];
            let start = place_contiguous(device, addr, w);
            slot_addr[slot] = start;
            addr = start + u16::from(w);
        }
        debug_assert_eq!(addr, size_end, "frame end moved for {}", f.name);
        for (name, &slot) in &fl.slot_of {
            let key = format!("{}::{name}", f.name);
            locals.insert(key.clone(), slot_addr[slot]);
            local_width.insert(key, fl.widths[slot]);
        }
    }

    // 7b. Per-bank high-water marks and the ISR region span. Every placed
    // address (globals + locals, both contexts) contributes its end; the
    // ISR region is the distance from the ISR root's base to the highest
    // ISR-context frame end, 0 without an ISR.
    let mut bank_used: Vec<u16> = device.ram_banks.iter().map(|_| 0u16).collect();
    let mut isr_bytes: u16 = 0;
    if !isr_names.is_empty() {
        let isr_roots: Vec<&String> = topo
            .iter()
            .filter(|f| !callers.contains_key(*f) && isr_names.contains(f.as_str()))
            .collect();
        let isr_ctx: HashSet<String> = isr_roots
            .iter()
            .flat_map(|r| reachable(&[r.as_str()], &edges))
            .collect();
        let isr_lo = isr_roots
            .iter()
            .map(|r| base[r.as_str()])
            .min()
            .unwrap_or(bank0_start);
        let isr_hi = isr_ctx
            .iter()
            .map(|f| frame_end(device, base[f], &locals_widths[f]))
            .max()
            .unwrap_or(isr_lo);
        isr_bytes = isr_hi - isr_lo;
    }
    for (i, &(start, end)) in device.ram_banks.iter().enumerate() {
        let mut hi: Option<u16> = None;
        for g in &m.globals {
            if let Some(&a) = globals.get(&g.name) {
                // A bank-straddling global's physical end skips common RAM
                // (docs/33 §D-2), so the per-bank high-water must use
                // physical_end, not a + size (which would overcount the
                // first bank into the common-RAM hole and miss the later
                // banks entirely).
                let e = physical_end(device, a, u16::from(g.size));
                if a >= start && a <= end {
                    hi = Some(hi.map_or(e, |h| h.max(e)));
                }
            }
        }
        for (key, &a) in &locals {
            let e = a + u16::from(local_width[key]);
            if a >= start && a <= end {
                hi = Some(hi.map_or(e, |h| h.max(e)));
            }
        }
        bank_used[i] = hi.map_or(0, |h| h - start);
    }

    // 8. Total bank-0 demand = max over roots of depth_end(root), with an
    // ISR root's disjoint base offset included (its region starts after the
    // main context's total, not at bank0_start). The arithmetic is relative
    // to the region start, so it reads the same whichever end of RAM the
    // overlay was placed at.
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
        bank_used,
        isr_bytes,
        has_isr: !isr_names.is_empty(),
        isr_low_save,
        isr_save,
        isr_hi_save,
    }
}

/// The value a defining instruction writes: `(name, byte width)`, or `None`
/// for non-defining instructions. `icmp` results are i1 (1 byte) regardless
/// of the operand type. `resolved` is iselcore's pointer resolution for
/// the module: a pointer select gets a slot only when iselcore seeded it
/// as an indirect slot (its two address bytes materialize into the dst).
/// A folded select is virtual (iselcore folds it like a GEP) and defines no
/// slot; allocating one would be dead space that perturbs the liveness
/// coloring and can clobber a fold-term register (epic-cc#117, epic-cc#147).
fn def_width(inst: &Inst, resolved: &PtrResolution, fname: &str) -> Option<(String, u8)> {
    match inst {
        Inst::Load(l) => Some((l.dst.clone(), l.ty.bytes())),
        Inst::Bin(b) => Some((b.dst.clone(), b.ty.bytes())),
        Inst::Zext(z) => Some((z.dst.clone(), z.to.bytes())),
        Inst::Sext(s) => Some((s.dst.clone(), s.to.bytes())),
        Inst::Trunc(t) => Some((t.dst.clone(), t.to.bytes())),
        Inst::IntToPtr(p) => Some((p.dst.clone(), p.to.bytes())),
        Inst::Icmp(i) => Some((i.dst.clone(), 1)),
        // A pointer select iselcore seeded as an indirect slot
        // (`Base::Slot(dst, true)`) materializes its two address bytes into
        // the dst slot, so the dst needs a slot like any 2-byte value. A
        // folded select is virtual and defines no slot
        // (epic-cc#117, epic-cc#147).
        Inst::Select(s)
            if matches!(
                resolved.get(&ssa_key(fname, &s.dst)),
                Some((Base::Slot(_, true), 0, t)) if t.is_empty()
            ) =>
        {
            Some((s.dst.clone(), s.ty.bytes()))
        }
        // A value select (i1/i8/i16/f32) copies the selected operand into
        // the dst slot like any other value.
        Inst::Select(s) if !s.ptr => Some((s.dst.clone(), s.ty.bytes())),
        Inst::Select(_) => None,
        Inst::Call(c) => match (&c.dst, &c.ty) {
            (Some(d), Some(t)) => Some((d.clone(), t.bytes())),
            _ => None,
        },
        Inst::Phi(p) => Some((p.dst.clone(), p.ty.bytes())),
        Inst::Store(_)
        | Inst::Ret(..)
        | Inst::Br(_)
        | Inst::BrCond(_)
        | Inst::Switch(_)
        | Inst::VaStart(_) => None,
        // Gep computes a virtual pointer address (isel turns it into FSR/INDF
        // or a RETLW table read); it defines no value needing a RAM slot.
        Inst::Gep(_) => None,
        // Alloca defines a size-byte local buffer (the slot is sized below
        // and in alloc); Memcpy defines nothing.
        Inst::Alloca(a) => Some((a.dst.clone(), a.size)),
        Inst::Memcpy(_) => None,
        // Freeze defines the dst slot, sized by the operand type.
        Inst::Freeze(f) => Some((f.dst.clone(), f.ty.bytes())),
        // va_arg defines the read argument's dst, sized by its type.
        Inst::VaArg(v) => Some((v.dst.clone(), v.ty.bytes())),
        // Float: binops/conv casts define an f32 (4-byte) dst; fcmp an i1.
        Inst::FloatBin(b) => Some((b.dst.clone(), 4)),
        Inst::Fcmp(c) => Some((c.dst.clone(), 1)),
        Inst::FloatConv(c) => Some((c.dst.clone(), c.to.bytes())),
        // Asm is opaque verbatim, defines no SSA value needing a RAM slot.
        Inst::Asm(_) => None,
    }
}

/// Render the layout as `global <name> 0xNN`, `const <name>` (no address:
/// the global lives in flash), and `local <func> <name> 0xNN` lines,
/// deterministically sorted by key. The internal alloc<->isel contract
/// (the alloc bin and alloc tests consume it); the driver's user-facing
/// map file renders the same facts with the unsplit `{func}::{name}` key
/// (driver::report::map_text).
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

    /// epic-cc#509: the PIC18 snap target is the next `0x100` boundary, not
    /// the next region's start, and a boundary that lands in an unmapped gap
    /// resolves forward into the region that actually contains it.
    /// `p18f2450` is the two-region PIC18 (`[[0x0010,0x01FF],[0x0400,0x04FF]]`),
    /// so a frame derived at the end of its first region snaps over the
    /// `0x200-0x3FF` hole. Reached only directly: `allocate` resolves a
    /// routine's slots through `place_contiguous`, which masks the gap value
    /// before it can show in the map, so no module-level test observes it.
    #[test]
    fn pic18_routine_base_snaps_over_an_unmapped_gap() {
        let dev = device::resolve("p18f2450").expect("p18f2450 is a known device");
        // A 22-byte frame (the widest routine) based at 0x1EE straddles the
        // 0x1FF region end; the next 0x100 boundary is 0x200, inside the gap.
        assert_eq!(routine_base(dev, 0x1EE, &[22]), 0x400);
        // A base whose frame fits its bank is untouched.
        assert_eq!(routine_base(dev, 0x1D0, &[22]), 0x1D0);
        // Single-region PIC18: base 0xEB + 22 crosses 0x100, so it snaps to
        // the next boundary, which is inside the same region.
        let dev = device::resolve("p18f4550").expect("p18f4550 is a known device");
        assert_eq!(routine_base(dev, 0xEB, &[22]), 0x100);
        assert_eq!(routine_base(dev, 0xE7, &[22]), 0xE7);
    }

    /// epic-cc#535: the reorder applies only when the frame straddles the
    /// window's end. A frame wholly inside the window is already bank-free
    /// byte for byte, so permuting it can only break the `MOVFF` copy runs it
    /// walks (the measured `math` regression); a frame wholly outside it is
    /// banked throughout and likewise gains nothing. The placement path only
    /// ever sees PIC18 frames based at `gpr_start` inside the window, so the
    /// boundary cases have no module-level expression and are pinned here.
    #[test]
    fn window_order_fires_only_for_a_straddling_frame() {
        let layout = |widths: Vec<u8>, heat: Vec<u32>| FrameLayout {
            size: widths.iter().map(|&w| u16::from(w)).sum(),
            widths,
            slot_of: HashMap::new(),
            heat,
        };
        // A window of [0x10, 0x60) with frames based at 0x10, the PIC18 shape.
        // Wholly inside: 1 + 2 = 3 bytes, ending at 0x13, well under 0x60.
        assert!(window_order(0x10, 0x60, &layout(vec![1, 2], vec![9, 1])).is_none());
        // Straddling: 0x58 + 0x10 = 0x68 past the window end, and the hotter
        // 2-byte slot (heat 9) must come first.
        let straddling = layout(vec![8, 8], vec![1, 9]);
        assert_eq!(
            window_order(0x58, 0x60, &straddling),
            Some(vec![1, 0]),
            "the hotter slot takes the window-resident low bytes"
        );
        // Wholly above the window: no frame byte is bank-free, so leave it.
        assert!(window_order(0x60, 0x60, &straddling).is_none());
    }
}
