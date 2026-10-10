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

use device::{Core, Device};
use ir::{Inst, Module};
use iselcore::{find_value_folds, resolve_pointers, ssa_key, Base, PtrResolution, ValueFolds};

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
    /// Consts staged through the shared `__const_stage` buffer instead of
    /// per-copies (epic-cc#790): isel stages exactly these at their use
    /// sites. Empty on cores that copy, and whenever the set would not
    /// pay for its buffer.
    pub staged_consts: HashSet<String>,

    /// Address-taken const globals (the pre-demotion `const_to_ram`
    /// candidate set, staged members included): the pooled string
    /// table's input. Flash-bound today; their uses resolve through
    /// the pool from epic-cc#816 on (epic-cc#815).
    pub address_taken_consts: HashSet<String>,

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
fn frame_layout(
    f: &ir::Func,
    resolved: &PtrResolution,
    va_size: u16,
    unplaced: &HashSet<String>,
    retval_homed: &HashSet<String>,
) -> FrameLayout {
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
    // Every def keeps its caller slot (epic-cc#830): dropping a homed
    // value's slot shrinks the caller frame, which rebases its callees and
    // spends more bank-select words than the deleted copies save.

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
    // A folded or forwarded producer (epic-cc#863) needs no slot: isel
    // reads the source or computes into the store destination, so no
    // interval enters coloring and no address leaves placement.
    let mut vals: Vec<(&String, (usize, u16), (usize, u16), u8, usize)> = Vec::new();
    for (v, &(d, p_d, w, o, mem)) in &defs {
        if unplaced.contains(v) {
            continue;
        }
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
    // A block reachable from itself runs more than once, so linear
    // dead-after cannot see dynamic-after uses across the back edge.
    let on_cycle = |mi: usize| -> bool {
        let mut stack: Vec<usize> = succ.get(&mi).cloned().unwrap_or_default();
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(n) = stack.pop() {
            if n == mi {
                return true;
            }
            if seen.insert(n) {
                stack.extend(succ.get(&n).cloned().unwrap_or_default());
            }
        }
        false
    };
    // Phi-edge coalescing (epic-cc#727): a phi dst reuses its incoming
    // slot on a dead-after join. The copy point touches both intervals
    // so plain disjointness never fires; the src test is positional.
    // GEP-extended uses already cover FSR-indexed reads of src. A merge
    // on a CFG cycle repeats the copy each iteration, which linear order
    // cannot see, so cyclic merges punt (docs/45 twin-differential gate
    // backstops the rest). The pair shares one slot index, so the heat
    // permutation below never splits it. Byval and retval stay out.
    let mut coalesce: HashMap<String, String> = HashMap::new();
    for (mi, b) in order.iter().enumerate() {
        for inst in &b.insts {
            if let ir::Inst::Phi(p) = inst {
                let Some(&(_, _, w_d, _, mem_d)) = defs.get(&p.dst) else {
                    continue;
                };
                if mem_d {
                    continue;
                }
                // Pointer phis seed indirect slots in iselcore, not plain
                // value slots, so they never pin.
                if p.ptr {
                    continue;
                }
                let mut src: Option<String> = None;
                let mut mixed = false;
                let mut edges: Vec<usize> = Vec::new();
                let mut src_edges: Vec<usize> = Vec::new();
                for (v, pred) in &p.incoming {
                    let pi = idx[pred.as_str()];
                    edges.push(pi);
                    match v {
                        ir::Val::Reg(r) => match defs.get(r) {
                            Some(&(_, _, w_s, _, mem_s)) if !mem_s && w_s == w_d => match &src {
                                None => src = Some(r.clone()),
                                Some(s) if s == r => {}
                                _ => mixed = true,
                            },
                            _ => mixed = true,
                        },
                        _ => {}
                    }
                    if matches!(v, ir::Val::Reg(r) if Some(r) == src.as_ref()) {
                        src_edges.push(pi);
                    }
                }
                let Some(s) = src else { continue };
                // A retval-homed incoming lives in the fixed region, which
                // liveness cannot see: sharing its slot would let a later
                // call clobber the phi dst invisibly. (epic-cc#738)
                if retval_homed.contains(&s) {
                    continue;
                }
                if mixed {
                    continue;
                }
                // Merge on a CFG cycle repeats the copy each iteration, and
                // linear dead-after cannot see the dynamic-after use, so
                // punt. Straight-line diamonds in loopy functions stay
                // eligible: only a cycle through this merge disqualifies.
                if on_cycle(mi) || edges.iter().any(|&pi| pi >= mi) {
                    continue;
                }
                let mut ok = true;
                for &pi in &edges {
                    if live_out[pi].contains(&s) {
                        ok = false;
                        break;
                    }
                }
                if let Some(us) = uses.get(&s) {
                    // Incoming uses sit exactly at the pred end; anything
                    // past the latest src-carrying edge is still live.
                    let mut latest: Option<(usize, u16)> = None;
                    for &pi in &src_edges {
                        let c = (pi, block_len[pi]);
                        latest = Some(latest.map_or(c, |e| e.max(c)));
                    }
                    if let Some(last) = latest {
                        if us.iter().any(|&u| u > last) {
                            ok = false;
                        }
                    }
                }
                if ok {
                    coalesce.insert(p.dst.clone(), s);
                }
            }
        }
    }

    // Straight-line range copies (epic-cc#739): freeze and the casts copy
    // lane 0 byte-for-byte through staged copy lanes, so a dead-after
    // source lets the destination share its slot base and isel skips the
    // self lanes. Same positional test as the phi pin: the copy point
    // touches both intervals, and a cycle repeats the copy. Upper cast
    // lanes fill in place and never stage, so base sharing stays exact.
    for (mi, b) in order.iter().enumerate() {
        for (pos, inst) in b.insts.iter().enumerate() {
            let (dst, src) = match inst {
                ir::Inst::Freeze(f) => (&f.dst, &f.val),
                ir::Inst::Zext(z) => (&z.dst, &z.val),
                ir::Inst::Sext(s) => (&s.dst, &s.val),
                ir::Inst::Trunc(t) => (&t.dst, &t.val),
                _ => continue,
            };
            let ir::Val::Reg(s) = src else { continue };
            // Same fixed-region hazard as the phi pin above. (epic-cc#738)
            if retval_homed.contains(s) {
                continue;
            }
            let Some(&(_, _, w_s, _, mem_s)) = defs.get(s) else {
                continue;
            };
            let Some(&(_, _, w_d, _, mem_d)) = defs.get(dst) else {
                continue;
            };
            if mem_s || mem_d {
                continue;
            }
            if matches!(inst, ir::Inst::Freeze(_)) && w_s != w_d {
                continue;
            }
            if on_cycle(mi) {
                continue;
            }
            if live_out[mi].contains(s) {
                continue;
            }
            let pt = (mi, pos as u16);
            if let Some(us) = uses.get(s) {
                if us.iter().any(|&u| u > pt) {
                    continue;
                }
            }
            coalesce.insert(dst.clone(), s.clone());
        }
    }
    // Bitmask-lane coalescing (epic-cc#763): an or-select or or-bool lane
    // destructively updates its accumulator (`acc |= mask` is one `BSF`),
    // so a dead-after accumulator lets the lane dst share its slot and
    // isel skips the establish copy. Same positional dead-after test as
    // the phi/copy pins: the lane point touches both intervals, and a
    // cycle repeats the lane. The shape mirrors isel-pic18's lane match;
    // a lane isel rejects still lowers generically (arms read before dst
    // writes), so a pin that never lanes stays sound.
    let lane_const = |v: &ir::Val| -> bool { matches!(v, ir::Val::Const(0) | ir::Val::Const(1)) };
    let lane_cmp = |c: &ir::Icmp| -> bool {
        if c.ty != ir::Ty::I8 || (c.pred != "eq" && c.pred != "ne") {
            return false;
        }
        match (&c.a, &c.b) {
            (ir::Val::Reg(_), k) => lane_const(k),
            (k, ir::Val::Reg(_)) => lane_const(k),
            _ => false,
        }
    };
    // A lane dst shares its accumulator slot only when every use of the
    // accumulator is at or before the lane point and nothing past it can
    // observe the clobber: no later linear use, nothing live out of the
    // block. A use before the lane reads the pre-lane value, so only
    // positions past the lane veto.
    let lane_pin = |acc: &str, dst: &str, pt: (usize, u16)| -> bool {
        let (Some(&(_, _, w_s, _, mem_s)), Some(&(_, _, w_d, _, mem_d))) =
            (defs.get(acc), defs.get(dst))
        else {
            return false;
        };
        if mem_s || mem_d || w_s != w_d {
            return false;
        }
        // Same fixed-region hazard as the phi pin above. (epic-cc#738)
        if retval_homed.contains(acc) {
            return false;
        }
        if unplaced.contains(acc) || unplaced.contains(dst) {
            return false;
        }
        if on_cycle(pt.0) {
            return false;
        }
        if live_out[pt.0].contains(acc) {
            return false;
        }
        if let Some(us) = uses.get(acc) {
            if us.iter().any(|&u| u > pt) {
                return false;
            }
        }
        true
    };
    for (mi, b) in order.iter().enumerate() {
        for (pos, inst) in b.insts.iter().enumerate() {
            match inst {
                ir::Inst::Select(s) => {
                    if s.ptr || s.ty != ir::Ty::I8 {
                        continue;
                    }
                    let ir::Val::Reg(cnd) = &s.cond else {
                        continue;
                    };
                    if uses.get(cnd).is_none_or(|u| u.len() != 1) {
                        continue;
                    }
                    let Some(&(ci_b, ci_p, _, _, _)) = defs.get(cnd) else {
                        continue;
                    };
                    if ci_b != mi || ci_p >= pos as u16 {
                        continue;
                    }
                    let ir::Inst::Icmp(c) = &b.insts[ci_p as usize] else {
                        continue;
                    };
                    if !lane_cmp(c) {
                        continue;
                    }
                    // Or-select: one arm is a single-use `Or` over the
                    // other arm, the same accumulator reg on both sides.
                    for (or_v, acc_v) in [(&s.a, &s.b), (&s.b, &s.a)] {
                        let (ir::Val::Reg(or_r), ir::Val::Reg(acc_r)) = (or_v, acc_v) else {
                            continue;
                        };
                        if uses.get(or_r).is_none_or(|u| u.len() != 1) {
                            continue;
                        }
                        let Some(&(oi_b, oi_p, _, _, _)) = defs.get(or_r) else {
                            continue;
                        };
                        if oi_b != mi || oi_p >= pos as u16 {
                            continue;
                        }
                        let ir::Inst::Bin(ob) = &b.insts[oi_p as usize] else {
                            continue;
                        };
                        if ob.op != ir::BinOp::Or || ob.ty != ir::Ty::I8 {
                            continue;
                        }
                        let mask = match (&ob.a, &ob.b) {
                            (ir::Val::Reg(a), ir::Val::Const(m)) if a == acc_r => *m,
                            (ir::Val::Const(m), ir::Val::Reg(a)) if a == acc_r => *m,
                            _ => continue,
                        };
                        if mask <= 0 || mask > 0xFF || (mask as u8).count_ones() != 1 {
                            continue;
                        }
                        if lane_pin(acc_r, &s.dst, (mi, pos as u16)) {
                            coalesce.insert(s.dst.clone(), acc_r.clone());
                            break;
                        }
                    }
                }
                ir::Inst::Bin(ob) => {
                    // Or-bool tail: bit 0 set exactly when the predicate
                    // holds, no select involved.
                    if ob.op != ir::BinOp::Or || ob.ty != ir::Ty::I8 {
                        continue;
                    }
                    let (ir::Val::Reg(acc_r), ir::Val::Reg(z_r)) = (&ob.a, &ob.b) else {
                        continue;
                    };
                    if uses.get(z_r).is_none_or(|u| u.len() != 1) {
                        continue;
                    }
                    let Some(&(zi_b, zi_p, _, _, _)) = defs.get(z_r) else {
                        continue;
                    };
                    if zi_b != mi || zi_p >= pos as u16 {
                        continue;
                    }
                    let ir::Inst::Zext(z) = &b.insts[zi_p as usize] else {
                        continue;
                    };
                    if z.from != ir::Ty::I1 || z.to != ir::Ty::I8 {
                        continue;
                    }
                    let ir::Val::Reg(cnd) = &z.val else {
                        continue;
                    };
                    if uses.get(cnd).is_none_or(|u| u.len() != 1) {
                        continue;
                    }
                    let Some(&(ci_b, ci_p, _, _, _)) = defs.get(cnd) else {
                        continue;
                    };
                    if ci_b != mi || ci_p >= pos as u16 {
                        continue;
                    }
                    let ir::Inst::Icmp(c) = &b.insts[ci_p as usize] else {
                        continue;
                    };
                    if !lane_cmp(c) {
                        continue;
                    }
                    if lane_pin(acc_r, &ob.dst, (mi, pos as u16)) {
                        coalesce.insert(ob.dst.clone(), acc_r.clone());
                    }
                }
                _ => {}
            }
        }
    }

    // Greedy first-fit coloring: reuse the lowest slot whose interval is
    // disjoint from the new value's; the slot's width grows to the widest
    // occupant.
    let mut slots: Vec<((usize, u16), (usize, u16), u8)> = Vec::new();
    let mut slot_of: HashMap<String, usize> = HashMap::new();
    let interval_of: HashMap<&str, ((usize, u16), (usize, u16))> = vals
        .iter()
        .map(|(v, lo, hi, _, _)| (v.as_str(), (*lo, *hi)))
        .collect();
    for (v, lo, hi, w, _) in &vals {
        if let Some(s) = coalesce.get((*v).as_str()) {
            if let Some(&si) = slot_of.get(s.as_str()) {
                // A second pair pinning the same slot falls back unless the
                // destination is disjoint from every other occupant; the
                // source overlap is covered by the dead-after guards above.
                let clash = slot_of.iter().any(|(o, &osi)| {
                    osi == si
                        && o != s
                        && match interval_of.get(o.as_str()) {
                            Some(&(olo, ohi)) => !(hi < &olo || &ohi < lo),
                            None => true,
                        }
                });
                if !clash {
                    slots[si].0 = slots[si].0.min(*lo);
                    slots[si].1 = slots[si].1.max(*hi);
                    slots[si].2 = slots[si].2.max(*w);
                    slot_of.insert((*v).clone(), si);
                    continue;
                }
            }
        }
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
        if unplaced.contains(v) {
            continue;
        }
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

/// Cross-block homing ownership (epic-cc#830): the def block dominates the
/// call block, both run at most once, and no block on any path between
/// holds a call. Any interleaving call could write an overlapping sibling
/// frame, so doubt rejects the site.
fn cross_block_ok(
    succ: &HashMap<usize, Vec<usize>>,
    order: &[&ir::Block],
    def_block: usize,
    def_pos: usize,
    call_block: usize,
    call_pos: usize,
) -> bool {
    let n = order.len();
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }
    // Iterative dominators from the entry; unreachable blocks keep the
    // full set, which admits nothing through them.
    let mut dom: Vec<HashSet<usize>> = vec![(0..n).collect(); n];
    dom[0] = HashSet::from([0]);
    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..n {
            let mut d: Option<HashSet<usize>> = None;
            for p in preds.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
                d = Some(match d {
                    None => dom[*p].clone(),
                    Some(d) => d.intersection(&dom[*p]).copied().collect(),
                });
            }
            if let Some(mut d) = d {
                d.insert(i);
                if d != dom[i] {
                    dom[i] = d;
                    changed = true;
                }
            }
        }
    }
    if !dom[call_block].contains(&def_block) {
        return false;
    }
    // At most once each: a repeated call would read a slot the callee
    // itself may have rewritten, and a repeated def needs path order the
    // dominance alone does not give inside a loop.
    let self_reachable = |x: usize| -> bool {
        let mut stack = succ.get(&x).cloned().unwrap_or_default();
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(b) = stack.pop() {
            if b == x {
                return true;
            }
            if seen.insert(b) {
                stack.extend(succ.get(&b).cloned().unwrap_or_default());
            }
        }
        false
    };
    if self_reachable(def_block) || self_reachable(call_block) {
        return false;
    }
    if order[def_block].insts[def_pos + 1..]
        .iter()
        .any(|x| matches!(x, ir::Inst::Call(_)))
    {
        return false;
    }
    if order[call_block].insts[..call_pos]
        .iter()
        .any(|x| matches!(x, ir::Inst::Call(_)))
    {
        return false;
    }
    // Blocks on some def-to-call path: forward-reachable from the def and
    // backward-reaching the call. Loops among them are covered because any
    // call inside rejects, whatever the iteration order.
    let mut fwd: HashSet<usize> = HashSet::from([def_block]);
    let mut stack = vec![def_block];
    while let Some(b) = stack.pop() {
        for s in succ.get(&b).cloned().unwrap_or_default() {
            if fwd.insert(s) {
                stack.push(s);
            }
        }
    }
    let mut bwd: HashSet<usize> = HashSet::from([call_block]);
    stack = vec![call_block];
    while let Some(b) = stack.pop() {
        for p in preds.get(&b).cloned().unwrap_or_default() {
            if bwd.insert(p) {
                stack.push(p);
            }
        }
    }
    for b in fwd.intersection(&bwd) {
        if *b == def_block || *b == call_block {
            continue;
        }
        if order[*b]
            .insts
            .iter()
            .any(|x| matches!(x, ir::Inst::Call(_)))
        {
            return false;
        }
    }
    true
}

/// Phi homing ownership (epic-cc#830): the phi and the call share the
/// merge block, every predecessor carries an incoming arm, the merge runs
/// at most once, and no block on any arm-to-call path holds a call.
/// Incoming values keep their own slots; only the copy destinations move
/// to the param slot, so the arms need no check beyond the path rule.
fn phi_home_ok(
    succ: &HashMap<usize, Vec<usize>>,
    order: &[&ir::Block],
    idx: &HashMap<&str, usize>,
    phi: &ir::Phi,
    merge: usize,
    phi_pos: usize,
    call_pos: usize,
) -> bool {
    if phi.incoming.is_empty() || phi_pos >= call_pos {
        return false;
    }
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }
    let merge_preds = preds.get(&merge).cloned().unwrap_or_default();
    for mp in &merge_preds {
        if !phi
            .incoming
            .iter()
            .any(|(_, pred)| idx.get(pred.as_str()) == Some(mp))
        {
            return false;
        }
    }
    let self_reachable = |x: usize| -> bool {
        let mut stack = succ.get(&x).cloned().unwrap_or_default();
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(b) = stack.pop() {
            if b == x {
                return true;
            }
            if seen.insert(b) {
                stack.extend(succ.get(&b).cloned().unwrap_or_default());
            }
        }
        false
    };
    if self_reachable(merge) {
        return false;
    }
    if order[merge].insts[..call_pos]
        .iter()
        .any(|x| matches!(x, ir::Inst::Call(_)))
    {
        return false;
    }
    let mut bwd: HashSet<usize> = HashSet::from([merge]);
    let mut stack = vec![merge];
    while let Some(b) = stack.pop() {
        for p in preds.get(&b).cloned().unwrap_or_default() {
            if bwd.insert(p) {
                stack.push(p);
            }
        }
    }
    for mp in &merge_preds {
        let mut fwd: HashSet<usize> = HashSet::from([*mp]);
        let mut stack = vec![*mp];
        while let Some(b) = stack.pop() {
            for s in succ.get(&b).cloned().unwrap_or_default() {
                if fwd.insert(s) {
                    stack.push(s);
                }
            }
        }
        for b in fwd.intersection(&bwd) {
            if *b == *mp || *b == merge {
                continue;
            }
            if order[*b]
                .insts
                .iter()
                .any(|x| matches!(x, ir::Inst::Call(_)))
            {
                return false;
            }
        }
    }
    true
}

/// Entry-path ownership for homed caller params (epic-cc#830): the call
/// runs at most once and no block on any entry-to-call path holds a call
/// (the call block contributes only its prefix: later calls read dead
/// bytes). Callers refresh the param slot at every call site, so only the
/// window from function entry needs proving.
fn entry_path_ok(
    succ: &HashMap<usize, Vec<usize>>,
    order: &[&ir::Block],
    call_block: usize,
    call_pos: usize,
) -> bool {
    let self_reachable = |x: usize| -> bool {
        let mut stack = succ.get(&x).cloned().unwrap_or_default();
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(b) = stack.pop() {
            if b == x {
                return true;
            }
            if seen.insert(b) {
                stack.extend(succ.get(&b).cloned().unwrap_or_default());
            }
        }
        false
    };
    if self_reachable(call_block) {
        return false;
    }
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }
    let mut fwd: HashSet<usize> = HashSet::from([0]);
    let mut stack = vec![0];
    while let Some(b) = stack.pop() {
        for s in succ.get(&b).cloned().unwrap_or_default() {
            if fwd.insert(s) {
                stack.push(s);
            }
        }
    }
    if !fwd.contains(&call_block) {
        return true;
    }
    let mut bwd: HashSet<usize> = HashSet::from([call_block]);
    stack = vec![call_block];
    while let Some(b) = stack.pop() {
        for p in preds.get(&b).cloned().unwrap_or_default() {
            if bwd.insert(p) {
                stack.push(p);
            }
        }
    }
    for b in fwd.intersection(&bwd) {
        if *b == call_block {
            if order[*b].insts[..call_pos]
                .iter()
                .any(|x| matches!(x, ir::Inst::Call(_)))
            {
                return false;
            }
            continue;
        }
        if order[*b]
            .insts
            .iter()
            .any(|x| matches!(x, ir::Inst::Call(_)))
        {
            return false;
        }
    }
    true
}
/// Iterative dominators over `order` (entry is block 0); unreachable blocks
/// keep the full set, which admits nothing through them. The arg-homing
/// windows above predate this helper and keep their inline copies.
fn dominators(succ: &HashMap<usize, Vec<usize>>, n: usize) -> Vec<HashSet<usize>> {
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }
    let mut dom: Vec<HashSet<usize>> = vec![(0..n).collect(); n];
    dom[0] = HashSet::from([0]);
    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..n {
            let mut d: Option<HashSet<usize>> = None;
            for p in preds.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
                d = Some(match d {
                    None => dom[*p].clone(),
                    Some(d) => d.intersection(&dom[*p]).copied().collect(),
                });
            }
            if let Some(mut d) = d {
                d.insert(i);
                if d != dom[i] {
                    dom[i] = d;
                    changed = true;
                }
            }
        }
    }
    dom
}

/// Result window for retval homing (epic-cc#738): no call and no inline asm
/// on any path from the defining call to its single read, either of which
/// could rewrite the fixed retval bytes in between. A same-block loop needs
/// no rejection: each iteration re-runs the call before the read, so a
/// back-edge clobber never reaches one.
fn result_home_ok(
    succ: &HashMap<usize, Vec<usize>>,
    order: &[&ir::Block],
    dom: &[HashSet<usize>],
    call_block: usize,
    call_pos: usize,
    use_block: usize,
    use_pos: usize,
) -> bool {
    let clobbers = |inst: &ir::Inst| matches!(inst, ir::Inst::Call(_) | ir::Inst::Asm(_));
    if use_block == call_block {
        if use_pos <= call_pos {
            return false;
        }
        return !order[call_block].insts[call_pos + 1..use_pos]
            .iter()
            .any(clobbers);
    }
    if !dom[use_block].contains(&call_block) {
        return false;
    }
    let self_reachable = |x: usize| -> bool {
        let mut stack = succ.get(&x).cloned().unwrap_or_default();
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(b) = stack.pop() {
            if b == x {
                return true;
            }
            if seen.insert(b) {
                stack.extend(succ.get(&b).cloned().unwrap_or_default());
            }
        }
        false
    };
    if self_reachable(call_block) || self_reachable(use_block) {
        return false;
    }
    if order[call_block].insts[call_pos + 1..].iter().any(clobbers) {
        return false;
    }
    if order[use_block].insts[..use_pos].iter().any(clobbers) {
        return false;
    }
    let mut preds: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, ss) in succ {
        for s in ss {
            preds.entry(*s).or_default().push(*i);
        }
    }
    let mut fwd: HashSet<usize> = HashSet::from([call_block]);
    let mut stack = vec![call_block];
    while let Some(b) = stack.pop() {
        for s in succ.get(&b).cloned().unwrap_or_default() {
            if fwd.insert(s) {
                stack.push(s);
            }
        }
    }
    let mut bwd: HashSet<usize> = HashSet::from([use_block]);
    stack = vec![use_block];
    while let Some(b) = stack.pop() {
        for p in preds.get(&b).cloned().unwrap_or_default() {
            if bwd.insert(p) {
                stack.push(p);
            }
        }
    }
    for b in fwd.intersection(&bwd) {
        if *b == call_block || *b == use_block {
            continue;
        }
        if order[*b].insts.iter().any(clobbers) {
            return false;
        }
    }
    true
}

/// Call results homed into the fixed retval region (epic-cc#738):
/// `(caller, value)` when the value is a call result with one read and a
/// clobber-free window to it. The result already lands in the retval bytes,
/// so leaving it there deletes the call-site copy: isel's self-copy skip
/// drops it once placement maps the dst onto the region. Only the copy at
/// the call site is deleted, so no frame rebases. The window proves no
/// interleaving call or asm; the ISR save area covers preemption.
fn home_results(
    m: &Module,
    device: &Device,
    resolved: &PtrResolution,
) -> HashSet<(String, String)> {
    let mut homed: HashSet<(String, String)> = HashSet::new();
    let Some((retval_lo, retval_hi)) = device.fixed_retval else {
        return homed;
    };
    debug_assert!(
        retval_hi - retval_lo + 1 >= 4,
        "alloc: fixed_retval must hold a 4-byte result"
    );
    for f in &m.funcs {
        let order = block_order(f);
        let idx: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.as_str(), i))
            .collect();
        let block_len: Vec<u16> = order.iter().map(|b| b.insts.len() as u16).collect();
        let mut uses: HashMap<String, HashSet<(usize, u16)>> = HashMap::new();
        for (i, b) in order.iter().enumerate() {
            for (pos, inst) in b.insts.iter().enumerate() {
                if let ir::Inst::Phi(p) = inst {
                    for (v, pred) in &p.incoming {
                        let vn = ir::val_name(v);
                        if vn.is_empty() {
                            continue;
                        }
                        let pi = idx[pred.as_str()];
                        uses.entry(vn).or_default().insert((pi, block_len[pi]));
                    }
                    continue;
                }
                for v in ir::read_vals(inst) {
                    if v.is_empty() {
                        continue;
                    }
                    uses.entry(v).or_default().insert((i, pos as u16));
                }
            }
        }
        // A GEP's base and term regs are re-read by isel at every load or
        // store through the derived pointer, so those reads count as uses
        // of the address value too. Without this a pointer result read
        // through a GEP past a later call would look single-use.
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
        let norm = |t: &str| {
            t.strip_prefix("label ")
                .unwrap_or(t)
                .trim_start_matches('%')
                .to_string()
        };
        let mut succ: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, ob) in order.iter().enumerate() {
            let mut ss = Vec::new();
            for inst in &ob.insts {
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
        let dom = dominators(&succ, order.len());
        for (bi, b) in order.iter().enumerate() {
            for (pos, inst) in b.insts.iter().enumerate() {
                let ir::Inst::Call(c) = inst else { continue };
                let (Some(d), Some(t)) = (&c.dst, &c.ty) else {
                    continue;
                };
                if !(1..=4).contains(&t.bytes()) {
                    continue;
                }
                if resolved.contains_key(&ssa_key(&f.name, d)) {
                    continue;
                }
                let Some(us) = uses.get(d) else { continue };
                if us.len() != 1 {
                    continue;
                }
                let &(ub, up) = us.iter().next().expect("alloc: single use");
                if (ub, up) == (bi, pos as u16) {
                    continue;
                }
                if (up as usize) < order[ub].insts.len()
                    && matches!(order[ub].insts[up as usize], ir::Inst::Asm(_))
                {
                    continue;
                }
                if result_home_ok(&succ, &order, &dom, bi, pos, ub, up as usize) {
                    homed.insert((f.name.clone(), d.clone()));
                }
            }
        }
    }
    homed
}

/// One admitted call site, before the same-caller selection. `def` is the
/// defining write (the merge block for a phi, whose real writes are the
/// per-edge copies, so `phi_preds` names those predecessor blocks). Two
/// candidates whose writes can land inside each other's window could
/// clobber one shared slot, so one of them is kept.
struct HomeCand {
    def: (usize, usize),
    call: (usize, usize),
    src: (String, String),
    tgt: (String, String),
    /// Write points when the def is a phi: its per-edge copies sit at the
    /// END of each predecessor block, not at the merge. Empty otherwise.
    phi_preds: Vec<(usize, usize)>,
}

/// Caller-computed call args (epic-cc#830): `(caller, value)` to
/// `(callee, param)` when the value's defining write can target the
/// callee's param slot, deleting the call-site copy. Sources are single-use
/// scalar regs (plain defs, call results, phis, caller params), width-equal,
/// with a clobber-free window the per-shape check proves: no call between,
/// callee out of ISR reach and unable to reach back. Sibling frames share
/// RAM, so any intervening call rejects. A retval-homed call result
/// (epic-cc#738) stays out: both passes would rewrite its address.
fn home_args(
    m: &Module,
    edges: &HashMap<String, Vec<String>>,
    resolved: &PtrResolution,
    retval_homed: &HashSet<(String, String)>,
) -> HashMap<(String, String), (String, String)> {
    let funcs: HashMap<&str, &ir::Func> = m.funcs.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut called_by: HashMap<&str, Vec<&str>> = HashMap::new();
    for (p, cs) in edges {
        for c in cs {
            called_by.entry(c.as_str()).or_default().push(p.as_str());
        }
    }
    let isr_reachable: HashSet<String> = m
        .funcs
        .iter()
        .filter(|f| f.isr && !called_by.contains_key(f.name.as_str()))
        .flat_map(|r| reachable(&[r.name.as_str()], edges))
        .collect();
    // A def isel emits as compute-then-store to the dst slot. A call
    // result lands the same way (retval bytes to the dst slot on both
    // cores), so chaining calls home too. Casts and freezes stay out:
    // the coalescer already folds a dead-after one into its source slot,
    // so homing only resurrects its copy at the param address.
    let homable = |inst: &Inst| -> bool {
        matches!(
            inst,
            Inst::Load(_) | Inst::Bin(_) | Inst::Icmp(_) | Inst::Call(_)
        ) || matches!(inst, Inst::Select(s) if !s.ptr)
    };
    // One writer per param slot at a time (epic-cc#830 review): sibling
    // callees' first params can alias one address, so a second homed def
    // reaching an earlier site's call would clobber the value that site
    // reads. Defs are not calls, so the per-site window checks cannot see
    // it; selection below is CFG-aware per caller.
    let mut cands: Vec<HomeCand> = Vec::new();
    let mut succ_of: HashMap<String, HashMap<usize, Vec<usize>>> = HashMap::new();
    for f in &m.funcs {
        let order = block_order(f);
        let idx: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.as_str(), i))
            .collect();
        let block_len: Vec<u16> = order.iter().map(|b| b.insts.len() as u16).collect();
        // Use positions mirror frame_layout's `uses`: phi incoming counts
        // at the predecessor end, so exactly one entry means the call arg
        // is the value's only read in any form.
        let mut uses: HashMap<String, Vec<(usize, u16)>> = HashMap::new();
        for (i, b) in order.iter().enumerate() {
            for (pos, inst) in b.insts.iter().enumerate() {
                if let ir::Inst::Phi(p) = inst {
                    for (v, pred) in &p.incoming {
                        let vn = ir::val_name(v);
                        if vn.is_empty() {
                            continue;
                        }
                        let pi = idx[pred.as_str()];
                        uses.entry(vn).or_default().push((pi, block_len[pi]));
                    }
                    continue;
                }
                for v in ir::read_vals(inst) {
                    if v.is_empty() {
                        continue;
                    }
                    uses.entry(v).or_default().push((i, pos as u16));
                }
                if let ir::Inst::Gep(g) = inst {
                    uses.entry(g.dst.clone()).or_default().push((i, pos as u16));
                }
            }
        }
        // Successor map for the cross-block ownership check below,
        // mirroring frame_layout's construction.
        let norm = |t: &str| {
            t.strip_prefix("label ")
                .unwrap_or(t)
                .trim_start_matches('%')
                .to_string()
        };
        let mut succ: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, ob) in order.iter().enumerate() {
            let mut ss = Vec::new();
            for inst in &ob.insts {
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
        succ_of.insert(f.name.clone(), succ.clone());
        let params: HashSet<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        for (bi, b) in order.iter().enumerate() {
            for (pos, inst) in b.insts.iter().enumerate() {
                let ir::Inst::Call(c) = inst else { continue };
                if !c.callees.is_empty() {
                    continue;
                }
                let Some(callee) = funcs.get(c.func.as_str()) else {
                    continue;
                };
                if callee.name == f.name {
                    continue;
                }
                if reachable(&[callee.name.as_str()], edges).contains(&f.name) {
                    continue;
                }
                if isr_reachable.contains(&callee.name) {
                    continue;
                }
                let named = callee.params.len();
                for (i, arg) in c.args.iter().enumerate() {
                    if i >= named {
                        continue;
                    }
                    let Some(aty) = arg.ty else { continue };
                    if arg.byval.is_some() || arg.sret {
                        continue;
                    }
                    let p = &callee.params[i];
                    if p.byval.is_some() || p.sret {
                        continue;
                    }
                    if p.width != aty.bytes() {
                        continue;
                    }
                    let ir::Val::Reg(r) = &arg.val else { continue };
                    if retval_homed.contains(&(f.name.clone(), r.clone())) {
                        continue;
                    }
                    // A caller param passed straight through homes like a
                    // def at entry: callers refresh its slot at every call
                    // site, so only the entry-to-call window needs proving.
                    // Byval/sret/pointer params route through custom
                    // emission, never a plain slot copy.
                    if params.contains(r.as_str()) {
                        let fp = f
                            .params
                            .iter()
                            .find(|x| x.name == *r)
                            .expect("alloc: param source");
                        if fp.byval.is_some() || fp.sret || fp.ptr {
                            continue;
                        }
                        if fp.width != aty.bytes() {
                            continue;
                        }
                        if resolved.contains_key(&ssa_key(&f.name, r)) {
                            continue;
                        }
                        if uses
                            .get(r)
                            .map_or(true, |u| u.len() != 1 || u[0] != (bi, pos as u16))
                        {
                            continue;
                        }
                        if !entry_path_ok(&succ, &order, bi, pos) {
                            continue;
                        }
                        cands.push(HomeCand {
                            def: (0, 0),
                            call: (bi, pos),
                            src: (f.name.clone(), r.clone()),
                            tgt: (callee.name.clone(), p.name.clone()),
                            phi_preds: Vec::new(),
                        });
                        continue;
                    }
                    if resolved.contains_key(&ssa_key(&f.name, r)) {
                        continue;
                    }
                    if uses
                        .get(r)
                        .map_or(true, |u| u.len() != 1 || u[0] != (bi, pos as u16))
                    {
                        continue;
                    }
                    // The single static def may sit in another block; a
                    // cross-block one needs the ownership check below.
                    let mut def: Option<(usize, usize, u8)> = None;
                    for (db, ob) in order.iter().enumerate() {
                        for (dp, dinst) in ob.insts.iter().enumerate() {
                            if let Some((n, w)) = def_width(dinst, resolved, &f.name) {
                                if n == *r {
                                    def = Some((db, dp, w));
                                }
                            }
                        }
                    }
                    let Some((db, dp, w)) = def else { continue };
                    if w != p.width {
                        continue;
                    }
                    let mut phi_pred_blocks: Vec<(usize, usize)> = Vec::new();
                    if let ir::Inst::Phi(phi) = &order[db].insts[dp] {
                        if db != bi || !phi_home_ok(&succ, &order, &idx, phi, bi, dp, pos) {
                            continue;
                        }
                        // The phi's real writes are its per-edge copies at
                        // the END of each predecessor block, not the merge
                        // point the def tuple names.
                        for (_, pred) in &phi.incoming {
                            if let Some(&pi) = idx.get(pred.as_str()) {
                                phi_pred_blocks.push((pi, block_len[pi] as usize));
                            }
                        }
                    } else {
                        if !homable(&order[db].insts[dp]) {
                            continue;
                        }
                        if db == bi {
                            if order[db].insts[dp + 1..pos]
                                .iter()
                                .any(|x| matches!(x, ir::Inst::Call(_)))
                            {
                                continue;
                            }
                        } else if !cross_block_ok(&succ, &order, db, dp, bi, pos) {
                            continue;
                        }
                    }
                    cands.push(HomeCand {
                        def: (db, dp),
                        call: (bi, pos),
                        src: (f.name.clone(), r.clone()),
                        tgt: (callee.name.clone(), p.name.clone()),
                        phi_preds: phi_pred_blocks,
                    });
                }
            }
        }
    }
    // Same-caller selection. A kept writer's read at its call must see its
    // own write, so no other writer that can clobber that address may sit
    // inside the window. Only two writers can share an address: the same
    // target slot, or two callee frames that can overlay (siblings do;
    // distinct params of one callee never do, all params being entry-live).
    // Collisions are judged on FINAL addresses, because a pass-through
    // chain sends a site's bytes to a deeper param slot.
    let imm: HashMap<(String, String), (String, String)> = cands
        .iter()
        .map(|c| (c.src.clone(), c.tgt.clone()))
        .collect();
    let final_of = |mut tgt: (String, String)| -> Option<(String, String)> {
        let mut seen: HashSet<(String, String)> = HashSet::from([tgt.clone()]);
        while let Some(next) = imm.get(&tgt) {
            if !seen.insert(next.clone()) {
                return None;
            }
            tgt = next.clone();
        }
        Some(tgt)
    };
    let mut sel: HashMap<(String, String), (String, String)> = HashMap::new();
    let mut kept: Vec<(&HomeCand, (String, String))> = Vec::new();
    'cand: for c in &cands {
        let Some(cfinal) = final_of(c.tgt.clone()) else {
            continue;
        };
        // A phi writes at its pred ends, an ordinary def at its own point.
        let writes = |x: &HomeCand| -> Vec<(usize, usize)> {
            if x.phi_preds.is_empty() {
                vec![x.def]
            } else {
                x.phi_preds.clone()
            }
        };
        let cw = writes(c);
        for (k, kfinal) in &kept {
            if k.src.0 != c.src.0 {
                continue;
            }
            // Two writers can share one address only when their frames can
            // overlay: different callees always can (sibling frames share
            // a base), two different params of one callee never can (all
            // params are entry-live, so coloring gives them distinct
            // slots). A chain link is not its param slot: its bytes go to
            // a deeper frame, so a link always goes to the window test.
            let unchained_distinct =
                c.tgt.0 == k.tgt.0 && c.tgt.1 != k.tgt.1 && c.tgt == cfinal && k.tgt == *kfinal;
            if unchained_distinct && c.tgt != k.tgt {
                continue;
            }
            let succ = &succ_of[&c.src.0];
            // Same-block order is inclusive: two writers recorded at the
            // same point (two params, a param and an entry-position def,
            // two phis sharing a predecessor) both write the slot, and
            // whichever runs second wins, so they collide.
            let reaches = |from: (usize, usize), to: (usize, usize)| -> bool {
                if from.0 == to.0 {
                    return from.1 <= to.1;
                }
                let mut seen: HashSet<usize> = HashSet::new();
                let mut stack = succ.get(&from.0).cloned().unwrap_or_default();
                while let Some(b) = stack.pop() {
                    if b == to.0 {
                        return true;
                    }
                    if seen.insert(b) {
                        stack.extend(succ.get(&b).cloned().unwrap_or_default());
                    }
                }
                false
            };
            // A violation is one candidate's write landing between the
            // other's write and its call: the reader would see the
            // clobberer instead of its own value. `mine` owns the window,
            // `theirs` is the intruder. The intruder only clobbers along a
            // path to the read that re-runs no `mine` write: a loop re-running
            // the owner's def overwrites the intruder before the read (`visible`).
            let kw = writes(k);
            let visible =
                |t: (usize, usize), call: (usize, usize), mine: &[(usize, usize)]| -> bool {
                    if t.0 == call.0 && t.1 <= call.1 {
                        return !mine.iter().any(|m| m.0 == t.0 && m.1 > t.1 && m.1 < call.1);
                    }
                    if mine.iter().any(|m| m.0 == t.0 && m.1 > t.1) {
                        return false;
                    }
                    let mut seen: HashSet<usize> = HashSet::new();
                    let mut stack = succ.get(&t.0).cloned().unwrap_or_default();
                    while let Some(b) = stack.pop() {
                        if b == call.0 {
                            if !mine.iter().any(|m| m.0 == b && m.1 < call.1) {
                                return true;
                            }
                            continue;
                        }
                        if mine.iter().any(|m| m.0 == b) || !seen.insert(b) {
                            continue;
                        }
                        stack.extend(succ.get(&b).cloned().unwrap_or_default());
                    }
                    false
                };
            let clobbered = |mine: &[(usize, usize)],
                             theirs: &[(usize, usize)],
                             call: (usize, usize)|
             -> bool {
                theirs
                    .iter()
                    .any(|t| mine.iter().any(|m| reaches(*m, *t)) && visible(*t, call, mine))
            };
            if clobbered(&kw, &cw, k.call) || clobbered(&cw, &kw, c.call) {
                continue 'cand;
            }
        }
        kept.push((c, cfinal));
        sel.insert(c.src.clone(), c.tgt.clone());
    }
    sel
}

/// Whole-program overlay address allocation (entry point used by the
/// driver, the `alloc` binary, and tests). On PIC18 with fitting frames
/// it pairs two passes (epic-cc#863): a bounding pass with every slot
/// placed, whose globals name the access-safe fold set, then the gated
/// pass that drops those slots. Classic PIC14 pairs the same two passes
/// (epic-cc#875), gated on every mapped RAM global: without an access
/// bank the fold only deletes file-register accesses, so no placement
/// can make one cost flash. Other cores, and PIC18 layouts that fall
/// back to globals-first, take the bounding pass as the result.
pub fn allocate(device: &Device, m: &Module, edges_text: &str) -> AllocLayout {
    allocate_with_pool(device, m, edges_text, false)
}

/// `allocate` with the pooled flash string table (epic-cc#816): when
/// `pool` is set on PIC14, a pooled const whose uses isel rewrites to
/// pool addresses keeps no RAM copy. The driver sets this from
/// `--const-pool`; every other caller keeps the status quo.
pub fn allocate_with_pool(
    device: &Device,
    m: &Module,
    edges_text: &str,
    pool: bool,
) -> AllocLayout {
    let (nofold, fit) = allocate_inner(device, m, edges_text, None, pool);
    if device.core == Core::Pic14 {
        // Globals-first is exact, not monotone: global placement reads
        // only the module's globals, never the frame widths the gated
        // pass shrinks, so the bounding map's globals are the final
        // map's globals and the two passes fold the same set.
        let safe: HashSet<String> = m
            .globals
            .iter()
            .filter(|g| !g.is_const && nofold.globals.contains_key(&g.name))
            .map(|g| g.name.clone())
            .collect();
        return allocate_inner(device, m, edges_text, Some(&safe), pool).0;
    }
    if device.core != Core::Pic18 || !fit {
        return nofold;
    }
    let Some((_, hi)) = device.access_bank else {
        return nofold;
    };
    // Same bank test `isel-pic18` applies to the final map, so its
    // folds agree with these drops on every placed slot.
    let safe: HashSet<String> = m
        .globals
        .iter()
        .filter(|g| !g.is_const && nofold.globals.get(&g.name).is_some_and(|a| *a <= hi))
        .map(|g| g.name.clone())
        .collect();
    allocate_inner(device, m, edges_text, Some(&safe), pool).0
}

/// Address allocation with a value-fold gate (epic-cc#863). `None`
/// places every slot; `Some` drops the access-safe folds it names. The
/// public `allocate` pairs a bounding pass with the gated one. The
/// `bool` reports whether the frames fit below the globals; when they
/// do not, the globals-first fallback owns the layout instead. `pool`
/// gates pooled-const RAM copies on PIC14 (epic-cc#816).
fn allocate_inner(
    device: &Device,
    m: &Module,
    edges_text: &str,
    gate: Option<&HashSet<String>>,
    pool: bool,
) -> (AllocLayout, bool) {
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
    // Value folds (epic-cc#863, epic-cc#875): producers needing no slot,
    // computed by the predicate both PIC18 and PIC14 backends share, so
    // a dropped slot is never read. Other cores keep every slot: their
    // backends still address staged copies. The gate names foldable
    // globals (access-safe on PIC18, every mapped RAM global on PIC14);
    // `None` folds nothing, for the bounding pass.
    let fold_core = device.core == Core::Pic18 || device.core == Core::Pic14;
    let folds: HashMap<String, ValueFolds> = if fold_core {
        m.funcs
            .iter()
            .map(|f| (f.name.clone(), find_value_folds(f, m, &resolved, gate)))
            .collect()
    } else {
        HashMap::new()
    };
    // PIC14 drops only what its backend skips: folded loads plus
    // load-forwarded stores. Forwarded `Bin` results keep their slot
    // there, so they stay out of this set even though the shared
    // predicate names them.
    let unplaced: HashMap<String, HashSet<String>> = m
        .funcs
        .iter()
        .map(|f| {
            let empty = ValueFolds::default();
            let folds = folds.get(&f.name).unwrap_or(&empty);
            let dropped = if device.core == Core::Pic14 {
                folds.unplaced_pic14(f)
            } else {
                folds.unplaced()
            };
            (f.name.clone(), dropped)
        })
        .collect();
    let no_fold: HashSet<String> = HashSet::new();
    let unplaced_in = |name: &str| unplaced.get(name).unwrap_or(&no_fold);

    // Steps 1-5 shape the frame overlay over a region whose start is still
    // open. Nothing here depends on where the globals land, which is what
    // lets PIC18 place the overlay BELOW them (step 5's caller).

    // 1. Call graph from the edge text. Parsed before the frames: the
    // arg-homing admit below reads it, and coloring reads the admit.
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
    // Call results homed into the fixed retval region (epic-cc#738), computed
    // before arg homing so chained results stay out of both passes. Frame
    // layout takes the per-function value names to keep coalescing off the
    // fixed region, whose clobbers liveness cannot see.
    let retval_homed = home_results(m, device, &resolved);
    let retval_in = |name: &str| -> HashSet<String> {
        retval_homed
            .iter()
            .filter(|(fun, _)| fun == name)
            .map(|(_, val)| val.clone())
            .collect()
    };
    // Caller-computed call args (epic-cc#830). The caller keeps its own
    // slot for the value: only the copy at the call site is deleted, so
    // no frame rebases. A homed target that is itself a homed source
    // (pass-through chains) resolves to the final address; mutual
    // pass-throughs with no base slot drop out.
    let homed = home_args(m, &edges, &resolved, &retval_homed);
    let mut final_tgt: HashMap<(String, String), (String, String)> = HashMap::new();
    for (src, mut tgt) in homed.clone() {
        let mut seen: HashSet<(String, String)> = HashSet::from([src.clone()]);
        while homed.contains_key(&tgt) && seen.insert(tgt.clone()) {
            tgt = homed[&tgt].clone();
        }
        if !homed.contains_key(&tgt) {
            final_tgt.insert(src, tgt);
        }
    }

    // 2. locals_widths(f) = the liveness-overlay slot widths of f's params
    // and defined values, in allocation order (the order `frame_end` walks
    // and the locals placement reproduces). Values whose live ranges never
    // overlap share a slot, so a frame shrinks from the width sum to the
    // peak simultaneous demand. locals_size(f) is the colored frame's byte
    // size (epic-cc#172).
    let mut locals_widths: HashMap<String, Vec<u8>> = HashMap::new();
    let mut locals_size: HashMap<String, u16> = HashMap::new();
    let va_sizes = va_sizes(m);
    for f in &m.funcs {
        let fl = frame_layout(
            f,
            &resolved,
            floored_va_size(f, &va_sizes),
            unplaced_in(&f.name),
            &retval_in(&f.name),
        );
        locals_widths.insert(f.name.clone(), fl.widths);
        locals_size.insert(f.name.clone(), fl.size);
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
    // The pool constructor's input (epic-cc#815): every address-taken
    // const, captured before staging demotes its members below.
    let mut address_taken_consts: HashSet<String> = HashSet::new();
    // Direct triggers: a const global named outright as a plain call arg
    // or an unfolding select arm. Derived triggers: reached through a
    // GEP/reg chain. The split feeds staging below.
    let mut const_direct: HashSet<String> = HashSet::new();
    let mut const_derived: HashSet<String> = HashSet::new();
    // Pool gating records (epic-cc#816): byval/sret direct uses keep
    // their copy, and every reg use's (func, reg) lets the gating pass
    // resolve it through iselcore instead of the syntactic chain above.
    let mut const_byval: HashSet<String> = HashSet::new();
    let mut const_reg_uses: HashMap<String, Vec<(String, String)>> = HashMap::new();
    // A const feeding a pointer phi (the LSR chase shape) reads back
    // through iselcore's indirect slot with RAM semantics, so it keeps
    // its copy regardless of its other uses (epic-cc#816).
    let mut const_phi: HashSet<String> = HashSet::new();
    // Every arm of a non-folding pointer select (epic-cc#816): the
    // select seeds an indirect slot isel derefs with RAM semantics,
    // so each const arm keeps its copy even when directly named.
    let mut const_sel: HashSet<String> = HashSet::new();
    // Staging site records (epic-cc#790): per-call direct consts and reg
    // args, per-select dst with direct const arms, operand users per
    // (func, reg) with block positions, direct call edges, and functions
    // referenced as values. The passes below demote sharing-unsafe
    // triggers back to copies.
    struct CallSite {
        func: String,
        callee: String,
        indirect: bool,
        direct: Vec<(usize, String)>,
        regs: Vec<(usize, String)>,
    }
    struct SelSite {
        func: String,
        block: usize,
        inst: usize,
        dst: String,
        arms: Vec<String>,
    }
    let mut calls: Vec<CallSite> = Vec::new();
    let mut sels: Vec<SelSite> = Vec::new();
    let mut users: HashMap<(String, String), Vec<(usize, usize, bool)>> = HashMap::new();
    let mut addr_taken: HashSet<String> = HashSet::new();
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
            for (bi, b) in f.blocks.iter().enumerate() {
                for (ii, inst) in b.insts.iter().enumerate() {
                    // Operand users and address-taken globals for the
                    // staging soundness rules below. Calls record their
                    // own arg kinds in the branch beneath; every other
                    // instruction contributes read_vals uses.
                    ir::collect_global_vals(inst, &mut addr_taken);
                    match inst {
                        ir::Inst::Gep(g) => {
                            if let ir::GepBase::Global(name) = &g.base {
                                addr_taken.insert(name.clone());
                            }
                        }
                        ir::Inst::Load(l) => {
                            if let Some(name) = l.ptr.strip_prefix('@') {
                                addr_taken.insert(name.to_string());
                            }
                        }
                        ir::Inst::Store(s) => {
                            if let Some(name) = s.ptr.strip_prefix('@') {
                                addr_taken.insert(name.to_string());
                            }
                            if let ir::Val::Global(g) = &s.val {
                                addr_taken.insert(g.clone());
                            }
                        }
                        ir::Inst::Asm(a) => {
                            for op in &a.operands {
                                if let Some(name) = op.ptr.strip_prefix('@') {
                                    addr_taken.insert(name.to_string());
                                }
                            }
                        }
                        ir::Inst::Call(_) => {}
                        _ => {}
                    }
                    // Every non-call instruction contributes read_vals uses
                    // (the call branch beneath records its own arg kinds).
                    // Gep/Load/Store/Asm regs count here too: a hidden extra
                    // use must break select single-use (epic-cc#790 review).
                    if !matches!(inst, ir::Inst::Call(_)) {
                        for v in ir::read_vals(inst) {
                            if !v.is_empty() {
                                users
                                    .entry((f.name.clone(), v))
                                    .or_default()
                                    .push((bi, ii, false));
                            }
                        }
                    }
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
                                                    // Byval/sret args copy
                                                    // bulk: never staging
                                                    // candidates (epic-cc#790).
                                                    if arg.byval.is_none() && !arg.sret {
                                                        const_direct.insert(g.clone());
                                                    } else {
                                                        const_byval.insert(g.clone());
                                                    }
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
                                                    const_to_ram.insert(base.clone());
                                                    const_derived.insert(base.clone());
                                                    const_reg_uses
                                                        .entry(base)
                                                        .or_default()
                                                        .push((f.name.clone(), r.clone()));
                                                }
                                            }
                                        }
                                    }
                                    ir::Val::Const(_) => {}
                                }
                            }
                        }
                    }
                    if let ir::Inst::Call(c) = inst {
                        // Staging site records (epic-cc#790): reg args for
                        // select-def lookup with their plain-arg kind,
                        // direct edges for ISR reachability, and the site.
                        let mut direct: Vec<(usize, String)> = Vec::new();
                        let mut regs: Vec<(usize, String)> = Vec::new();
                        for (i, arg) in c.args.iter().enumerate() {
                            let plain = arg.ty.is_none() && arg.byval.is_none() && !arg.sret;
                            match &arg.val {
                                ir::Val::Global(g) => {
                                    if plain
                                        && m.globals.iter().any(|gl| &gl.name == g && gl.is_const)
                                    {
                                        direct.push((i, g.clone()));
                                    }
                                }
                                ir::Val::Reg(r) => {
                                    users
                                        .entry((f.name.clone(), r.clone()))
                                        .or_default()
                                        .push((bi, ii, plain));
                                    if plain {
                                        regs.push((i, r.clone()));
                                    }
                                }
                                ir::Val::Const(_) => {}
                            }
                        }
                        if !c.callees.is_empty() {
                            users
                                .entry((f.name.clone(), c.func.clone()))
                                .or_default()
                                .push((bi, ii, false));
                        }
                        calls.push(CallSite {
                            func: f.name.clone(),
                            callee: c.func.clone(),
                            indirect: !c.callees.is_empty(),
                            direct,
                            regs,
                        });
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
                        let arms = [
                            (const_base(&s.a), matches!(s.a, ir::Val::Global(_))),
                            (const_base(&s.b), matches!(s.b, ir::Val::Global(_))),
                        ];
                        if arms[0].0 != arms[1].0 {
                            let mut direct_arms: Vec<String> = Vec::new();
                            for ((base, is_direct), v) in arms.into_iter().zip([&s.a, &s.b]) {
                                let Some(g) = base else { continue };
                                // A GEP/reg-derived arm needs a real address
                                // and keeps a RAM copy; a directly named arm
                                // stages on small cores (below). Either way
                                // the arm reads back through the select's
                                // indirect slot, so gating never drops it.
                                if let Some(gl) = m.globals.iter().find(|gl| gl.name == g) {
                                    if gl.size <= 255 {
                                        const_to_ram.insert(g.clone());
                                        const_sel.insert(g.clone());
                                        if is_direct {
                                            const_direct.insert(g.clone());
                                            direct_arms.push(g.clone());
                                        } else {
                                            const_derived.insert(g.clone());
                                            if let ir::Val::Reg(r) = v {
                                                const_reg_uses
                                                    .entry(g)
                                                    .or_default()
                                                    .push((f.name.clone(), r.clone()));
                                            }
                                        }
                                    }
                                }
                            }
                            if !direct_arms.is_empty() {
                                sels.push(SelSite {
                                    func: f.name.clone(),
                                    block: bi,
                                    inst: ii,
                                    dst: s.dst.clone(),
                                    arms: direct_arms,
                                });
                            }
                        }
                    }
                    // A const reaching a pointer phi keeps its RAM copy:
                    // iselcore seeds the phi as an indirect slot and every
                    // read through it derefs RAM (the LSR chase shape whose
                    // flash lowering is epic-cc#811). A gated address here
                    // would deref flash as RAM.
                    if let ir::Inst::Phi(p) = inst {
                        if p.ptr {
                            for (v, _) in &p.incoming {
                                let base = match v {
                                    ir::Val::Global(g) => {
                                        if m.globals.iter().any(|gl| &gl.name == g && gl.is_const) {
                                            Some(g.clone())
                                        } else {
                                            None
                                        }
                                    }
                                    ir::Val::Reg(r) => find_const_base(r),
                                    ir::Val::Const(_) => None,
                                };
                                if let Some(g) = base {
                                    const_phi.insert(g);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // Staging eligibility (epic-cc#790): directly triggered consts share
    // one buffer instead of per-copies, unless sharing is unsafe. Unsafe:
    // multi-value calls, orphan selects, ISR or address-taken uses, lone
    // consts (a copy is cheaper than a buffer), escaping callees (Pass C).
    // isel stages the explicit set below. Pinned consts keep their copy.
    let small_core = matches!(device.core, device::Core::Pic14 | device::Core::Pic14e);
    let mut staged: HashSet<String> = HashSet::new();
    let mut stage_max: u16 = 0;
    if small_core {
        let func_idx: HashMap<&str, usize> = m
            .funcs
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.as_str(), i))
            .collect();
        // Functions excluded from staging: ISR handlers, everything they
        // can reach over direct calls, and any address-taken function
        // (which an ISR dispatch could target through a pointer).
        let mut excluded: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = m
            .funcs
            .iter()
            .filter(|f| f.isr)
            .map(|f| f.name.clone())
            .collect();
        while let Some(f) = stack.pop() {
            if !excluded.insert(f.clone()) {
                continue;
            }
            if let Some(cs) = edges.get(&f) {
                stack.extend(cs.iter().cloned());
            }
        }
        for g in &addr_taken {
            if func_idx.contains_key(g.as_str()) {
                excluded.insert(g.clone());
            }
        }
        // Reg definitions that are candidate selects, for resolving call
        // args back to their staging select.
        let mut sel_of: HashMap<(String, String), usize> = HashMap::new();
        for (i, s) in sels.iter().enumerate() {
            sel_of.insert((s.func.clone(), s.dst.clone()), i);
        }
        // Demoted consts fall back to per-copies (already in const_to_ram).
        let mut demote: HashSet<String> = HashSet::new();
        // Triggers in excluded functions never stage.
        for c in &calls {
            if excluded.contains(&c.func) {
                demote.extend(c.direct.iter().map(|(_, g)| g.clone()));
            }
        }
        for s in &sels {
            if excluded.contains(&s.func) {
                demote.extend(s.arms.iter().cloned());
            }
        }
        // Pass A (calls): one call staging two values would leave both
        // params reading the last copy. Count distinct direct consts plus
        // one per feeding candidate select (a select stages at most one
        // arm per execution); two or more demotes everything involved.
        // Same-const repeats stage identical bytes and stay.
        for c in &calls {
            if excluded.contains(&c.func) {
                continue;
            }
            let mut units: HashSet<String> = HashSet::new();
            let mut sel_units: HashSet<usize> = HashSet::new();
            for (_, d) in &c.direct {
                units.insert(d.clone());
            }
            for (_, r) in &c.regs {
                if let Some(&si) = sel_of.get(&(c.func.clone(), r.clone())) {
                    sel_units.insert(si);
                }
            }
            let mut foreign = 0;
            for &si in &sel_units {
                if sels[si].arms.iter().any(|a| !units.contains(a)) {
                    foreign += 1;
                }
            }
            if units.len() + foreign >= 2 {
                demote.extend(units);
                for &si in &sel_units {
                    demote.extend(sels[si].arms.iter().cloned());
                }
            }
        }
        // Pass B (selects): a select stages its arms only with its single
        // consuming call in the same block, ordered after it, and no other
        // call or select between (any of which could re-stage first).
        for s in &sels {
            if s.arms.iter().all(|a| demote.contains(a)) {
                continue;
            }
            let mut ok = !excluded.contains(&s.func);
            if ok {
                match users.get(&(s.func.clone(), s.dst.clone())) {
                    Some(us) if us.len() == 1 => {
                        let (ub, ui, plain) = us[0];
                        ok = plain && ub == s.block && ui > s.inst;
                        if ok {
                            let blocks = &m.funcs[func_idx[s.func.as_str()]].blocks;
                            ok = !blocks[s.block].insts[s.inst + 1..ui]
                                .iter()
                                .any(|i| matches!(i, ir::Inst::Call(_) | ir::Inst::Select(_)));
                        }
                    }
                    _ => ok = false,
                }
            }
            if !ok {
                demote.extend(s.arms.iter().cloned());
            }
        }
        // Pass C (callee escape): the buffer is re-copied before every
        // consuming call, so a callee keeping the address alive (store,
        // return, forward) would read a clobbered buffer. Taint staged
        // params through GEPs, selects, and phis; dereferences and address
        // compares consume, any other tainted read escapes. Indirect calls
        // demote (unknown callee).
        for c in &calls {
            if excluded.contains(&c.func) {
                continue;
            }
            let mut involved: Vec<String> = Vec::new();
            let mut taint_args: Vec<usize> = Vec::new();
            for (i, g) in &c.direct {
                if !demote.contains(g) {
                    involved.push(g.clone());
                    taint_args.push(*i);
                }
            }
            for (i, r) in &c.regs {
                if let Some(&si) = sel_of.get(&(c.func.clone(), r.clone())) {
                    if sels[si].arms.iter().any(|a| !demote.contains(a)) {
                        involved.extend(
                            sels[si]
                                .arms
                                .iter()
                                .filter(|a| !demote.contains(*a))
                                .cloned(),
                        );
                        taint_args.push(*i);
                    }
                }
            }
            if involved.is_empty() {
                continue;
            }
            let mut escape = c.indirect;
            if !escape {
                if let Some(&fi) = func_idx.get(c.callee.as_str()) {
                    let callee = &m.funcs[fi];
                    let mut tainted: HashSet<String> = HashSet::new();
                    for &i in &taint_args {
                        if let Some(p) = callee.params.get(i) {
                            tainted.insert(p.name.clone());
                        }
                    }
                    let mut changed = true;
                    while changed && !tainted.is_empty() {
                        changed = false;
                        for b in &callee.blocks {
                            for inst in &b.insts {
                                let hit = |v: &ir::Val| matches!(v, ir::Val::Reg(r) if tainted.contains(r));
                                let dst: Option<&str> = match inst {
                                    ir::Inst::Gep(g) => {
                                        let used = matches!(&g.base, ir::GepBase::Reg(r) if tainted.contains(r))
                                            || g.terms.iter().any(|(_, r)| tainted.contains(r));
                                        if used {
                                            Some(g.dst.as_str())
                                        } else {
                                            None
                                        }
                                    }
                                    ir::Inst::Select(s) => {
                                        if hit(&s.a) || hit(&s.b) {
                                            Some(s.dst.as_str())
                                        } else {
                                            None
                                        }
                                    }
                                    ir::Inst::Phi(p) => {
                                        if p.incoming.iter().any(|(v, _)| hit(v)) {
                                            Some(p.dst.as_str())
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                };
                                if let Some(d) = dst {
                                    changed |= tainted.insert(d.to_string());
                                }
                            }
                        }
                    }
                    'scan: for b in &callee.blocks {
                        for inst in &b.insts {
                            let safe: Vec<String> = match inst {
                                ir::Inst::Load(l) => {
                                    vec![l.ptr.strip_prefix('%').unwrap_or(&l.ptr).to_string()]
                                }
                                ir::Inst::Store(s) => {
                                    vec![s.ptr.strip_prefix('%').unwrap_or(&s.ptr).to_string()]
                                }
                                ir::Inst::Select(s) => {
                                    vec![ir::val_name(&s.a), ir::val_name(&s.b)]
                                }
                                ir::Inst::Phi(p) => {
                                    p.incoming.iter().map(|(v, _)| ir::val_name(v)).collect()
                                }
                                ir::Inst::Gep(g) => {
                                    let mut vs = Vec::new();
                                    if let ir::GepBase::Reg(r) = &g.base {
                                        vs.push(r.clone());
                                    }
                                    vs.extend(g.terms.iter().map(|(_, r)| r.clone()));
                                    vs
                                }
                                ir::Inst::Memcpy(m) => {
                                    vec![ir::val_name(&m.dst), ir::val_name(&m.src)]
                                }
                                ir::Inst::Icmp(_) | ir::Inst::Fcmp(_) => ir::read_vals(inst),
                                _ => Vec::new(),
                            };
                            if ir::read_vals(inst)
                                .iter()
                                .any(|u| !u.is_empty() && tainted.contains(u) && !safe.contains(u))
                            {
                                escape = true;
                                break 'scan;
                            }
                        }
                    }
                } else {
                    escape = true;
                }
            }
            if escape {
                demote.extend(involved);
            }
        }

        // Survivors stage: direct-only, unpinned, small enough, paying.
        for gname in &const_direct {
            if const_derived.contains(gname) || demote.contains(gname) {
                continue;
            }
            if let Some(gl) = m.globals.iter().find(|gl| &gl.name == gname) {
                if gl.addr.is_none() && gl.size <= 255 {
                    staged.insert(gname.clone());
                    stage_max = stage_max.max(gl.size);
                }
            }
        }
        address_taken_consts = const_to_ram.clone();
        if staged.len() < 2 {
            staged.clear();
            stage_max = 0;
        } else {
            for gname in &staged {
                const_to_ram.remove(gname);
            }
        }
    }
    // Pool copy gating (epic-cc#816): a pooled const whose uses isel
    // rewrites to pool addresses needs no RAM copy. Rewritable uses
    // are direct call args and clean reg args resolving to the const
    // with no dynamic terms; a byval/sret arg, a dynamic term, a
    // select arm, a phi incoming, or any other opaque slot-mediated
    // flow keeps the copy, as does any mix of rewritable and RAM
    // uses. Only the PIC14 backend rewrites, so other cores keep
    // every copy.
    if pool && device.core == Core::Pic14 {
        // Pool membership as isel decides it (epic-cc#817): the
        // address-taken snapshot the driver feeds `build_pool`,
        // filtered by the shared eligibility predicate.
        let pooled = |n: &str| -> bool {
            address_taken_consts.contains(n)
                && m.globals
                    .iter()
                    .find(|gl| gl.name == n)
                    .is_some_and(iselcore::pool_member)
        };
        let ram_use = |g: &str| -> bool {
            if const_byval.contains(g) || const_phi.contains(g) {
                return true;
            }
            if const_sel.contains(g) {
                // A select arm stays RAM unless every selecting use
                // routes to the pool log variant (epic-cc#817); any
                // non-routed select pins the copy.
                for f in &m.funcs {
                    for b in &f.blocks {
                        for inst in &b.insts {
                            if let ir::Inst::Select(s) = inst {
                                if !s.ptr {
                                    continue;
                                }
                                let hits = |v: &ir::Val| -> bool {
                                    match v {
                                        ir::Val::Global(n) => n.as_str() == g,
                                        ir::Val::Reg(r) => {
                                            matches!(resolved.get(&ssa_key(&f.name, r)),
                                                Some((Base::Global(c), _, t)) if c.as_str() == g && t.is_empty())
                                        }
                                        _ => false,
                                    }
                                };
                                if !hits(&s.a) && !hits(&s.b) {
                                    continue;
                                }
                                if !iselcore::select_pool_routed(
                                    m, &resolved, &pooled, &f.name, &s.dst,
                                ) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
            match const_reg_uses.get(g) {
                None => false,
                Some(uses) => uses
                    .iter()
                    .any(|(func, r)| match resolved.get(&ssa_key(func, r)) {
                        Some((Base::Global(c), _, t)) => c != g || !t.is_empty(),
                        _ => true,
                    }),
            }
        };
        let mut gated: Vec<String> = Vec::new();
        // Staged consts left `const_to_ram` in the block above, so the
        // candidates are the union: a staged direct-only const gates out
        // of staging too, not just out of its copy.
        let mut cands: Vec<&String> = const_to_ram.iter().collect();
        cands.extend(staged.iter());
        for g in cands {
            let pooled = m
                .globals
                .iter()
                .find(|gl| &gl.name == g)
                .is_some_and(iselcore::pool_member);
            if pooled && !ram_use(g) {
                gated.push(g.clone());
            }
        }
        for g in &gated {
            const_to_ram.remove(g);
            staged.remove(g);
        }
        if staged.len() < 2 {
            const_to_ram.extend(staged.iter().cloned());
            staged.clear();
            stage_max = 0;
        } else {
            stage_max = staged
                .iter()
                .filter_map(|g| m.globals.iter().find(|gl| &gl.name == g))
                .map(|gl| gl.size)
                .max()
                .unwrap_or(0);
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
    // The shared staging buffer: one mutable RAM global sized to the
    // largest staged string. Every staged use re-copies before its call,
    // so the buffer is never read stale and needs no startup init (its
    // zero bytes already report no init).
    let mut stage_holder: Vec<ir::Global> = Vec::new();
    if stage_max > 0 {
        stage_holder.push(ir::Global {
            name: "__const_stage".to_string(),
            ty: ir::Ty::I8,
            is_const: false,
            size: stage_max,
            bytes: vec![0u8; stage_max as usize],
            refs: Vec::new(),
            addr: None,
        });
        floating.extend(stage_holder.iter());
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
    // Value-fold placement check (epic-cc#863, epic-cc#875): every
    // dropped slot's globals must be addressable in the final map, or
    // the backend declines the fold and reads a slot that is not there.
    // The gated pass only shrinks the overlay, so globals never move up
    // out of reach; a violation panics rather than miscompiling.
    if gate.is_some() {
        let hi = device.access_bank.map(|(_, hi)| hi).unwrap_or(u16::MAX);
        let placed = |g: &str| globals.get(g).is_some_and(|a| *a <= hi);
        for folds in folds.values() {
            for fold in folds.loads.values() {
                match fold {
                    iselcore::LoadFold::Direct(g) => {
                        assert!(
                            placed(g),
                            "alloc: folded source @{g} left addressable range"
                        );
                    }
                    iselcore::LoadFold::ThreadW(g, dsts) => {
                        assert!(
                            placed(g),
                            "alloc: threaded source @{g} left addressable range"
                        );
                        for d in dsts {
                            assert!(
                                placed(d),
                                "alloc: threaded store @{d} left addressable range"
                            );
                        }
                    }
                }
            }
            for dst in folds.forwarded.values() {
                assert!(
                    globals.contains_key(dst),
                    "alloc: forwarded store to unmapped @{dst}"
                );
            }
        }
    }

    // end_of_globals = max over the address map of the physical end (addr +
    // width, or the bank-straddling physical end on PIC14E), floored at the
    // device's GPR start (mirrors isel's layout computation). The
    // scratch/retval bytes live in the device's fixed common RAM, so the
    // first frame base follows the globals directly.
    let end_of_globals = m.globals.iter().chain(stage_holder.iter()).fold(
        device.gpr_start(),
        |end, g| match globals.get(&g.name) {
            Some(&a) => end.max(physical_end(device, a, u16::from(g.size))),
            None => end,
        },
    );

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
        let fl = frame_layout(
            f,
            &resolved,
            floored_va_size(f, &va_sizes),
            unplaced_in(&f.name),
            &retval_in(&f.name),
        );
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
    // Homed call args take the resolved callee param slot address: the
    // defining write lands there, so the site copy is a self-copy.
    // Widths match by admit, so one address covers every byte. Bank
    // pricing (epic-cc#849): without an access bank a site whose caller
    // and param slots sit in different banks is rejected. A same-bank
    // site cannot gain a BANKSEL (the def's bank trace is unchanged and
    // deleting the copy only removes selects). Rejection changes no
    // frame and rebases no callee: every def keeps its caller slot.
    let priced: Vec<((String, String), (String, String))> = final_tgt
        .into_iter()
        .filter(|((caller, val), (callee, param))| {
            if device.access_bank.is_some() {
                return true;
            }
            let src = locals
                .get(&format!("{caller}::{val}"))
                .copied()
                .unwrap_or_else(|| panic!("alloc: homed source {caller}::{val} has no slot"));
            let dst = locals
                .get(&format!("{callee}::{param}"))
                .copied()
                .unwrap_or_else(|| panic!("alloc: homed target {callee}::{param} has no slot"));
            device.bank_of(src) == device.bank_of(dst)
        })
        .collect();
    for ((caller, val), (callee, param)) in &priced {
        let target = format!("{callee}::{param}");
        let addr = locals
            .get(&target)
            .copied()
            .unwrap_or_else(|| panic!("alloc: homed target {target} has no slot"));
        let key = format!("{caller}::{val}");
        locals.insert(key.clone(), addr);
        local_width.insert(key, local_width[&target]);
    }
    // Homed call results take the fixed retval base: the defining call lands
    // there, so the site copy is a self-copy. Widths match by admit, so one
    // base covers every byte, and the PIC18 access bank needs no select. The
    // frame slot stays allocated, so no frame rebases. (epic-cc#738)
    if let Some((retval_lo, _)) = device.fixed_retval {
        for (caller, val) in &retval_homed {
            let key = format!("{caller}::{val}");
            if !locals.contains_key(&key) {
                panic!("alloc: retval-homed {key} has no slot");
            }
            locals.insert(key, retval_lo);
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
        // The synthetic staging buffer lives outside `m.globals`: chain
        // it in exactly like `end_of_globals` does, or the report
        // undercounts RAM by the buffer.
        for g in m.globals.iter().chain(stage_holder.iter()) {
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

    (
        AllocLayout {
            globals,
            locals,
            total_bank0,
            const_globals,
            staged_consts: staged,
            address_taken_consts,
            bank_used,
            isr_bytes,
            has_isr: !isr_names.is_empty(),
            isr_low_save,
            isr_save,
            isr_hi_save,
        },
        frames_fit,
    )
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
        // Only reached by direct `allocate` callers on raw IR: the driver legalizes
        // first, so these widths size the dst slot only when legalize is skipped.
        Inst::FloatBin(b) => Some((b.dst.clone(), 4)),
        Inst::Fcmp(c) => Some((c.dst.clone(), 1)),
        Inst::FloatConv(c) => Some((c.dst.clone(), c.to.bytes())),
        // Asm is opaque verbatim, defines no SSA value needing a RAM slot.
        Inst::Asm(_) => None,
    }
}

/// Render the layout as `global <name> 0xNN`, `const <name>` (no address:
/// the global lives in flash), `staged <name>` (shared-buffer staging),
/// and `local <func> <name> 0xNN` lines, deterministically sorted by key.
/// The internal alloc<->isel contract (the alloc bin and alloc tests
/// consume it); the driver's user-facing
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
    let mut staged: Vec<&String> = l.staged_consts.iter().collect();
    staged.sort();
    for name in staged {
        out.push_str(&format!("staged {name}\n"));
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
