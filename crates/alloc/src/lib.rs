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
use iselcore::{resolve_pointers, ssa_key, Base};

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
fn round_if_routine(
    device: &Device,
    f: &str,
    base: u16,
    locals_widths: &HashMap<String, Vec<u8>>,
) -> u16 {
    if ir::is_runtime_routine(f) {
        routine_base(device, base, &locals_widths[f])
    } else {
        base
    }
}

/// The liveness-colored frame of one function: the distinct slot widths in
/// allocation order (the order `frame_end` walks) and the frame's byte size
/// (the colored peak, not the width sum). Values whose live ranges never
/// overlap share a slot, so a frame shrinks from the width sum to the peak
/// simultaneous demand (M3 deferred this; epic-cc#172 is the deferral's
/// bill). The coloring is deterministic: values are processed in (range
/// start, placement order) and each reuses the lowest slot whose interval
/// is disjoint.
struct FrameLayout {
    widths: Vec<u8>,
    size: u16,
    /// value name -> slot index into `widths`. Every def and param gets a
    /// slot; the locals placement reads this to put each value at its
    /// slot's address.
    slot_of: HashMap<String, usize>,
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
    resolved: &HashMap<String, (Base, u8, Vec<(u8, String)>)>,
) -> FrameLayout {
    // Block order: the entry block (unlabeled) first, then label order.
    let mut order: Vec<&ir::Block> = f.blocks.iter().collect();
    // The entry block (label `entry` in hand-written IR, a numeric label in
    // irparse output) is always first; the rest follow in label order.
    order.sort_by_key(|b| match b.label.parse::<u64>() {
        Ok(v) => (1u8, v),
        Err(_) => (0u8, 0),
    });
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
                    let vn = val_name(v);
                    let pi = idx[pred.as_str()];
                    uses.entry(vn).or_default().insert((pi, block_len[pi]));
                }
                continue;
            }
            for v in inst_vals(inst) {
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
    let size: u16 = widths.iter().map(|&w| u16::from(w)).sum();
    FrameLayout {
        widths,
        size,
        slot_of,
    }
}

/// The SSA values an instruction reads, for liveness. Mirrors the operand
/// shapes of every `Inst` variant; a value that is only defined (never read)
/// contributes no use.
fn inst_vals(inst: &ir::Inst) -> Vec<String> {
    use ir::Inst;
    match inst {
        // Load/Store pointers are canonical prefixed forms (`%x`/`@g`);
        // strip the prefix so a local pointer matches the defs keys (a
        // global pointer is never a def and is filtered by the caller).
        Inst::Load(l) => vec![l.ptr.strip_prefix('%').unwrap_or(&l.ptr).to_string()],
        Inst::Store(s) => vec![
            s.ptr.strip_prefix('%').unwrap_or(&s.ptr).to_string(),
            val_name(&s.val),
        ],
        Inst::Bin(b) => vec![val_name(&b.a), val_name(&b.b)],
        Inst::Ret(Some((_, v))) => vec![val_name(v)],
        Inst::Ret(None) => Vec::new(),
        Inst::Zext(z) => vec![val_name(&z.val)],
        Inst::Sext(s) => vec![val_name(&s.val)],
        Inst::Trunc(t) => vec![val_name(&t.val)],
        Inst::IntToPtr(p) => vec![val_name(&p.val)],
        Inst::Icmp(i) => vec![val_name(&i.a), val_name(&i.b)],
        Inst::Select(s) => vec![val_name(&s.cond), val_name(&s.a), val_name(&s.b)],
        Inst::Call(c) => {
            let mut vs: Vec<String> = c.args.iter().map(|a| val_name(&a.val)).collect();
            // An indirect call's `func` is the SSA register holding the
            // function pointer; isel reads it at dispatch time (after the
            // args are loaded), so it is a use here. A direct call's `func`
            // is a function name, never a def key, and is filtered by the
            // caller.
            if !c.callees.is_empty() {
                vs.push(c.func.clone());
            }
            vs
        }
        Inst::Br(_) => Vec::new(),
        Inst::BrCond(b) => vec![val_name(&b.cond)],
        Inst::Phi(p) => p.incoming.iter().map(|(v, _)| val_name(v)).collect(),
        Inst::Gep(g) => {
            let mut vs = Vec::new();
            if let ir::GepBase::Reg(r) = &g.base {
                vs.push(r.clone());
            }
            vs.extend(g.terms.iter().map(|(_, r)| r.clone()));
            vs
        }
        Inst::Alloca(_) => Vec::new(),
        Inst::Memcpy(m) => vec![val_name(&m.dst), val_name(&m.src)]
            .into_iter()
            .chain(match &m.len {
                ir::MemLen::Const(_) => None,
                ir::MemLen::Reg(v) => Some(val_name(v)),
            })
            .collect(),
        Inst::Freeze(f) => vec![val_name(&f.val)],
        Inst::FloatBin(b) => vec![val_name(&b.a), val_name(&b.b)],
        Inst::Fcmp(c) => vec![val_name(&c.a), val_name(&c.b)],
        Inst::FloatConv(c) => vec![val_name(&c.val)],
        // Asm operand pointers are canonical prefixed forms (`%x`/`@g`);
        // strip the prefix so a local operand matches the defs keys (a
        // global operand is never a def and is filtered by the caller).
        Inst::Asm(a) => a
            .operands
            .iter()
            .map(|o| o.ptr.strip_prefix('%').unwrap_or(&o.ptr).to_string())
            .collect(),
    }
}

/// The SSA value name of a `Val` operand, or empty for a constant/global
/// (constants and globals are not frame locals).
fn val_name(v: &ir::Val) -> String {
    match v {
        ir::Val::Reg(r) => r.clone(),
        ir::Val::Const(_) | ir::Val::Global(_) => String::new(),
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

/// Largest-first bin-pack of the floating globals into per-bank cursors
/// (the fallback the sequential path used when it could not fit them all).
/// Independent per-bank cursors let a later small global use a bank the
/// sequential cursor already passed. Returns `None` when no arrangement
/// fits.
fn bin_pack(
    device: &Device,
    fixed: &[(String, u16, u16)],
    floating: &[&ir::Global],
) -> Option<HashMap<String, u16>> {
    let mut cursors: Vec<BankCursor> = device
        .ram_banks
        .iter()
        .map(|&(s, e)| BankCursor {
            end: e,
            next_free: s,
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
        let width = g.size as u8;
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
/// lines are informational). Panics loudly on a cyclic or unknown-function
/// call graph, and if total demand exceeds the device's GPR space.
pub fn allocate(device: &Device, m: &Module, edges_text: &str) -> AllocLayout {
    // iselcore's pointer resolution: a pointer select seeded as an indirect
    // slot materializes its two address bytes into the dst slot, so the
    // dst needs a RAM slot; a folded select is virtual and defines none.
    let resolved = resolve_pointers(m);
    // 1. Globals: sequential, aligned to at most two bytes (i16 -> even
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
                    // (iselcore seeds it as an
                    // indirect slot, epic-cc#147): the selected arm's bytes
                    // are read through the slot with RAM semantics, so each
                    // const arm must be copied to RAM. A select that folds
                    // (same base, e.g. the ccp_sel shape) keeps its const in
                    // flash: loads lower via the fold's RETLW/TBLRD path.
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
    for g in &m.globals {
        if g.is_const && !const_to_ram.contains(&g.name) {
            const_globals.insert(g.name.clone());
        } else {
            assert!(
                g.size <= 255,
                "alloc: RAM global @{} too large ({} bytes; RAM is byte-addressed, max 255)",
                g.name,
                g.size
            );
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
        // Prefer the arrangement with the lower footprint. The sequential
        // order preserves the .ll order, but a large global after small
        // ones wastes the tail of the first bank (epic-taskmgr's 80-byte
        // task table lands in bank 1 on the 877A, pushing the frames past
        // the last bank, epic-hal#86). The largest-first bin-pack closes
        // that gap; when both fit, the tighter end wins and the layout
        // stays otherwise unchanged (small fixtures place identically).
        let bin = bin_pack(device, &fixed, &floating);
        let floating_map: HashMap<String, u16> = match (&seq, &bin) {
            (Some(s), Some(b)) if global_end(b, &floating) < global_end(s, &floating) => b.clone(),
            (Some(s), _) => s.clone(),
            (None, Some(b)) => b.clone(),
            (None, None) => {
                let demand: u32 = floating.iter().map(|g| u32::from(g.size)).sum::<u32>()
                    + fixed.iter().map(|(_, _, s)| u32::from(*s)).sum::<u32>();
                let capacity: u32 = device
                    .ram_banks
                    .iter()
                    .map(|&(s, e)| u32::from(e) - u32::from(s) + 1)
                    .sum();
                let bank_count = device.ram_banks.len();
                panic!(
                    "alloc: no arrangement of {} global(s) fits {}'s {bank_count} GPR bank window(s) \
                     (total demand {demand} bytes, total capacity {capacity} bytes, no arrangement this \
                     allocator tries, sequential then largest-first bin-packing, fits)",
                    floating.len() + fixed.len(),
                    device.name,
                );
            }
        };
        floating_map
    };
    globals.extend(floating_map);

    // end_of_globals = max over the address map of (addr + width), floored at
    // the device's GPR start (mirrors isel's layout computation). The
    // scratch/retval bytes live in the device's fixed common RAM, so the
    // first frame base follows the globals directly.
    let end_of_globals =
        m.globals
            .iter()
            .fold(device.gpr_start(), |end, g| match globals.get(&g.name) {
                Some(&a) => end.max(a + u16::from(g.size)),
                None => end,
            });
    let bank0_start = end_of_globals;

    // 2. locals_widths(f) = the liveness-colored slot widths of f's params
    // and defined values, in allocation order (the order `frame_end` walks
    // and the locals placement reproduces). Values whose live ranges never
    // overlap share a slot, so a frame shrinks from the width sum to the
    // peak simultaneous demand (M3 deferred this; epic-cc#172 is the
    // deferral's bill). locals_size(f) is the colored frame's byte size.
    let mut locals_widths: HashMap<String, Vec<u8>> = HashMap::new();
    let mut locals_size: HashMap<String, u16> = HashMap::new();
    for f in &m.funcs {
        let fl = frame_layout(f, &resolved);
        locals_widths.insert(f.name.clone(), fl.widths);
        locals_size.insert(f.name.clone(), fl.size);
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
    let isr_names: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr)
        .map(|f| f.name.as_str())
        .collect();
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
        let isr_ctx: HashSet<String> = isr_roots
            .iter()
            .flat_map(|r| reachable(&[r.as_str()], &edges))
            .collect();
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

    // 7. Local addresses: each slot of the liveness-colored frame at the
    // next free frame byte, stepping through the banks via
    // `place_contiguous`/`region_for` (which panics if a frame exceeds the
    // device's last GPR bank, 0x1EF on PIC16F877A), then every value at its
    // slot's address. The slot walk is exactly `frame_end`'s, so the placed
    // end equals the physical end the callee bases were derived from.
    let mut locals: HashMap<String, u16> = HashMap::new();
    let mut local_width: HashMap<String, u8> = HashMap::new();
    for f in &m.funcs {
        let b = base[&f.name];
        let fl = frame_layout(f, &resolved);
        let mut slot_addr: Vec<u16> = Vec::with_capacity(fl.widths.len());
        let mut addr = b;
        for &w in &fl.widths {
            let start = place_contiguous(device, addr, w);
            slot_addr.push(start);
            addr = start + u16::from(w);
        }
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
                let e = a + u16::from(g.size);
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
        bank_used,
        isr_bytes,
        has_isr: !isr_names.is_empty(),
    }
}

/// The value a defining instruction writes: `(name, byte width)`, or `None`
/// for non-defining instructions. `icmp` results are i1 (1 byte) regardless
/// of the operand type. `resolved` is iselcore's pointer resolution for
/// the module: a pointer select gets a slot only when iselcore seeded it
/// as an indirect slot (its two address bytes are materialized into the
/// dst, epic-cc#117 and epic-cc#147). A folded select is virtual (iselcore
/// folds it like a GEP) and defines no slot; allocating one would be dead
/// space that perturbs the liveness coloring and can clobber a fold-term
/// register the fold still reads at every load site.
fn def_width(
    inst: &Inst,
    resolved: &HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    fname: &str,
) -> Option<(String, u8)> {
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
        // the dst slot (epic-cc#117 and epic-cc#147), so the dst needs a
        // slot like any 2-byte value. A folded select is virtual and
        // defines no slot.
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
}
