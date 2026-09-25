//! `isel-pic18`: instruction selection for the PIC18 integer spine,
//! extended by the pointer lowering (pointers, arrays, structs) and
//! the flash const (`const` in flash via `TBLRD`). See docs/29 §4
//! for phasing, ADR-009, and ADR-010. `const` globals read via
//! `TBLRD`: the 511-byte `RETLW` ceiling of PIC14 is gone, and
//! stores through a `const` base panic: ROM is not writable.
//! A separate crate from `isel` per docs/29 §2: the instruction sets
//! differ enough that sharing leaks an abstraction and risks PIC14.

use std::collections::{HashMap, HashSet};

use device::Device;
use ir::{Block, Func, Icmp, Inst, Module, SrcLoc, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, PtrResolution, Slot};

/// The high Access Bank segment's start: every classic-mode PIC18's SFRs
/// live at `0xF60-0xFFF` (160 bytes) by the core's own linear-addressing
/// definition (gputils' `.lkr` scripts declare it as a second, fixed
/// `ACCESSBANK` region, `accesssfr`, on both devices this core ships).
/// Unlike the low segment's `0x00-0x5F` (`Device::access_bank`, real
/// per-device data), the schema has no field for this because nothing has
/// needed one: it is architecture, not silicon. (epic-cc#226)
const PIC18_SFR_ACCESS_LO: u16 = 0xF60;

/// A straight `MOVFF` copy costs 2 words per byte; a seeded POSTINC copy
/// loop costs 9 words for any length (two 2-word LFSRs, MOVLW, the 2-word
/// POSTINC pair, DECFSZ, BRA), so it only pays from 5 bytes on. The floor
/// is 6: at 5 the loop wins one word while executing roughly 3x slower
/// per byte. (epic-cc#486)
const COPY_LOOP_MIN_PAIRS: usize = 6;

/// The result of resolving a pointer to a concrete access. `Direct`: the
/// address is statically known, so a plain `MOVFF`/`MOVF`/`MOVWF` reaches
/// it. `Indirect`: `FSR0` has been set up and the access goes through
/// `INDF0`.
#[derive(Clone, Copy)]
enum Addr {
    Direct(u16),
    Indirect,
}

/// Which scaled-term form won: the hardware multiplier, or the
/// shift-add chain. (epic-cc#477)
#[derive(Clone, Copy)]
enum Scaled {
    Mulwf,
    Chain,
}
/// A byte access completable through `PLUSW0` (`FSR0L/H` = 0xFE9/0xFEA,
/// `PLUSW0` = 0xFEB): the static part rides one `LFSR`, the byte index
/// rides `W`. Valid only for small static arrays (see `plusw_shape`).
/// (epic-cc#665)
struct PluswShape {
    idx_slot: u16,
    base: u16,
    k: u16,
}

/// A memcpy source's resolved shape: a plain address, an FSR1-indirect
/// pointer, or a flash table read through `TBLPTR`/`TABLAT`. Byte 0's
/// setup reports it so later bytes can walk instead of re-seeding.
/// (epic-cc#492)
#[derive(Clone, Copy)]
enum McSrc {
    Direct(u16),
    Indirect,
    Tblrd,
}

/// Identifies what an `emit_fsr0_dynamic`/`emit_fsr0_indirect_slot` setup
/// grounds FSR0 in, for the cross-access reuse check in `fsr0_holds`
/// (epic-cc#472). Two setups are the "same base" only when this matches
/// exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fsr0Origin {
    /// A compile-time-constant base address (a global's link-time
    /// address, or an alloca/byval slot's own fixed address) --
    /// `emit_fsr0_dynamic`'s `base_addr` parameter.
    Absolute(u16),
    /// A runtime pointer VALUE loaded fresh from the 2-byte slot at this
    /// address -- `emit_fsr0_indirect_slot`'s `slot_addr` parameter.
    /// Reuse is only sound while nothing has written to this slot since
    /// the value was loaded.
    SlotValue(u16),
}

struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)` by `iselcore::resolve_pointers`; see
    /// ADR-009.
    resolved: &'m PtrResolution,
    /// Fixed, `BSR`-independent return-value region (up to 4 bytes, from `device.fixed_retval`).
    /// Holds call results independent of the bank selection.
    retval_lo: u16,
    /// The access bank's high bound (from `device.access_bank`), soft-float
    /// runtime routines' frame-fits-in-the-access-bank assertions bound
    /// against. Every classic PIC18 device has shared this exact value
    /// (`0x5F`) so far, but it is device data, not an
    /// architectural constant guaranteed for every future PIC18 part. (epic-cc#226)
    access_bank_hi: u16,
    /// The `BSR` value the last-emitted `MOVLB` set, or `None` when it's
    /// unknown (module start, or just after a label: branch targets can
    /// be reached with any prior `BSR` state, so it must be re-established
    /// on the next banked access rather than assumed).
    bsr: Option<u8>,
    /// Forward-only join states for internal (fresh) labels, keyed by
    /// label: `Some(v)` when every recorded branch to the label left
    /// `BSR` at `v`, `None` once any disagrees. Central `emit` records
    /// every single-operand branch to a `tmp` label; block labels never
    /// match, loop bodies bypass `emit` through raw pushes or stay
    /// unknown by the loop-header audit below. A back-edge re-creates
    /// an entry after its label's decision, which is never consulted
    /// again, so it cannot contribute.
    fwd_join: HashMap<String, Option<u8>>,
    /// Whether `operand()` selected a bank since the flag was last
    /// cleared. Terminator lowerings with per-edge phi copies can leave
    /// each exit holding a different bank, so a block whose terminator
    /// lowering selected any bank records unknown instead of its end
    /// state. Straight-line prefix selects affect every exit equally:
    /// the flag resets immediately before each terminator lowering.
    bsr_dirty: bool,
    /// Callee name -> the bank every return path leaves live, from the
    /// buffered reverse-topological emission. A caller relies on an entry
    /// only after the map proved it from the code as actually emitted;
    /// absent or `None` entries clear the tracked bank exactly as master.
    exit_banks: &'m HashMap<String, Option<u8>>,
    /// What FSR0 currently addresses, if known: `Some((origin, offset))`
    /// where `offset` is the `k + byte_off` most recently set up on top of
    /// `origin`. Lets a later access through the *same* base skip
    /// re-deriving the address from scratch and instead add only the
    /// forward delta (epic-cc#472). `None` when unknown: module start,
    /// just after a label (mirrors `bsr`'s exact reasoning: a branch
    /// target can be reached with any prior FSR0 state), just after a
    /// `CALL` (the callee almost certainly used FSR0 for its own pointer
    /// accesses), or just after this codegen itself writes to a tracked
    /// `SlotValue`'s slot (the one case straight-line code can change the
    /// pointer value an already-resolved access still depends on -- see
    /// `invalidate_fsr0_if_slot_written`).
    fsr0_holds: Option<(Fsr0Origin, u16)>,
    /// Direct-to-direct `MOVFF` byte copies staged by `emit_copy_byte`,
    /// drained as straight MOVFFs or, once long enough, as one
    /// LFSR-seeded POSTINC copy loop (epic-cc#486). Each entry carries
    /// the source location active when it was staged so the parallel
    /// `locs` vector stays index-aligned whichever way the drain goes.
    pending_copies: Vec<(u16, u16, Option<SrcLoc>)>,
    /// Every RAM address a global occupies. The W cache never records or
    /// reuses one: an interrupt can write a global between the store and
    /// the reload, while the ISR epilogue restores W to its pre-interrupt
    /// value, so eliding the read would return the stale byte. Only a
    /// function-private frame slot is safe to track, the same structural
    /// rule PIC14's cache states (epic-cc#214).
    global_addrs: &'m HashSet<u16>,
    /// The GPR slot whose byte `emit_w_store`/`emit_w_load` last left in
    /// W, or `None` when unknown. When it is set, a reload of the same
    /// slot folds away and a memory-to-memory copy out of it becomes one
    /// `MOVWF` instead of a 2-word `MOVFF` (epic-cc#502).
    ///
    /// Only a slot's own byte is ever recorded: an SFR or literal address
    /// is read through the plain `emit` arms, so no hardware register's
    /// readback can be elided, and the IR's missing volatile flag needs
    /// no separate rule (epic-cc#214's argument, restated for this core).
    /// Every other emission path clears it, so a stale belief never
    /// survives a flag-setting, W-clobbering or label-joining instruction.
    /// `MOVF f,W` also sets STATUS Z from the byte it loaded, which
    /// `MOVWF` does not, so a reload whose Z a branch consumes passes
    /// `need_z` and is never elided.
    w_holds: Option<u16>,
    cur_func: &'m str,
    /// Marks an interrupt handler: the body runs a save prologue and restore
    /// epilogue with `RETFIE` instead of `RETURN` (the single-vector mode).
    isr: bool,
    /// Shares one module-scoped label counter across functions so `tmp{n}:`
    /// labels stay unique in the single output. Mirrors the `isel` counter.
    tmp: &'m mut u32,
    /// The source location of the instruction currently being emitted, or
    /// `None` for compiler-generated glue (prologue, `__start`, const
    /// tables, runtime routines). `emit` records it on the line it pushes,
    /// so the parallel `locs` vector stays index-aligned with `out`.
    cur_loc: Option<SrcLoc>,
    out: Vec<String>,
    /// One source location per emitted line, index-aligned with `out`.
    /// `None` marks a compiler-generated line (no source instruction).
    locs: Vec<Option<SrcLoc>>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.flush_copies();
        // Any instruction this sink did not route through `emit_w_store`/
        // `emit_w_load` may move W or set a flag, so the cache cannot
        // survive it (epic-cc#502).
        self.w_holds = None;
        let line = s.into();
        if let Some(t) = Self::fwd_target(&line) {
            // A user function literally named `tmp` plus digits would
            // collide with fresh labels; tail calls to one must not
            // record. The check runs only on tmp-shaped targets, so the
            // linear scan never touches the hot path.
            if !self.is_function(t) {
                let t = t.to_string();
                self.note_branch(&t);
            }
        }
        self.out.push(line);
        self.locs.push(self.cur_loc.clone());
    }

    /// `MOVWF f`, unless the byte at `f` already equals W from an
    /// immediately preceding `emit_w_store`/`emit_w_load` of the same
    /// address (a genuine no-op then). Marks `f` as holding W either way.
    ///
    /// The operand is resolved only when the store is actually emitted:
    /// `operand` can emit a `MOVLB`, and any emission clears the cache,
    /// so resolving on the cache-hit path would both waste the bank
    /// decision and destroy the belief it was about to reuse.
    fn emit_w_store(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            let (a, f) = self.operand(addr);
            let bank = if a == 0 { "A" } else { "B" };
            self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
        }
        self.mark_w(addr);
    }

    /// Record that W holds the byte at `addr`, unless the address belongs
    /// to a global: an ISR can rewrite a global at any time and its
    /// epilogue restores the interrupted W, so a global's cached byte is
    /// never trustworthy across the next instruction boundary.
    fn mark_w(&mut self, addr: u16) {
        self.w_holds = if self.global_addrs.contains(&addr) {
            None
        } else {
            Some(addr)
        };
    }

    /// `MOVF f,W`, unless W is already known to hold exactly this byte
    /// from an immediately preceding `emit_w_store`/`emit_w_load` of the
    /// same address (epic-cc#502).
    ///
    /// `need_z` marks a site whose next instruction consumes STATUS Z set
    /// by this load (the `BrCond`/`Select`/switch selectors). `MOVWF`
    /// leaves Z alone, so eliding the load there would branch on a stale
    /// flag; `need_z` forces the `MOVF`, which is what sets Z.
    fn emit_w_load(&mut self, addr: u16, need_z: bool) {
        if !need_z && self.w_holds == Some(addr) {
            return;
        }
        let (a, f) = self.operand(addr);
        let bank = if a == 0 { "A" } else { "B" };
        self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
        // `MOVF` with a file source and d=0 sets Z from the byte it read,
        // which is exactly the flag a following `BZ` wants.
        self.mark_w(addr);
    }

    /// Memory-to-memory byte copy, preferring the one-word `MOVWF` when W
    /// already holds the source byte (epic-cc#502 measured this shape as
    /// `MOVWF f` followed by `MOVFF f,g`: three words where one does).
    /// Falls back to the staged `emit_copy_byte` otherwise, so a long run
    /// of ordinary copies still drains as one LFSR-seeded loop.
    fn emit_copy_byte_or_w(&mut self, src: u16, dst: u16) {
        if self.w_holds == Some(src) && !self.global_addrs.contains(&src) {
            // The shortcut still writes `dst`, and a byte write to a
            // pointer slot must invalidate a tracked FSR0 grounded in it;
            // `emit_copy_byte` is otherwise the only place that does.
            self.invalidate_fsr0_if_slot_written(dst, 1);
            self.emit_w_store(dst);
            return;
        }
        self.emit_copy_byte(src, dst);
    }

    /// Drains `pending_copies`. A run that reached `COPY_LOOP_MIN_PAIRS`
    /// (the buffer only extends while each pair is consecutive with the
    /// first, src and dst each advancing by one) lowers to the seeded
    /// loop; anything shorter replays as the straight MOVFFs it would
    /// have been. The loop label is a pure straight-line cycle (reached
    /// only by the MOVLW above and the BRA below, body free of banked
    /// operands), so the tracked `bsr` survives it; the POSTINC walk does
    /// move FSR0 n bytes past its seed, so the tracked FSR0 position does
    /// not. Raw pushes, not `emit`: flush runs from inside `emit`.
    fn flush_copies(&mut self) {
        if self.pending_copies.is_empty() {
            return;
        }
        let pairs = std::mem::take(&mut self.pending_copies);
        let n = pairs.len();
        // One memcpy is bounded to 255 bytes by irparse, but chained
        // adjacent copies can stage a longer run; a byte-counted loop
        // cannot hold that count in the MOVLW literal, so long runs
        // replay straight, the pre-loop form.
        if n >= COPY_LOOP_MIN_PAIRS && n <= 255 {
            // The count rides in WREG, so the drained form clobbers W
            // (epic-cc#502).
            self.w_holds = None;
            let (src0, dst0, loc) = &pairs[0];
            let (src0, dst0) = (*src0, *dst0);
            let l_loop = self.fresh_label();
            for (text, line_loc) in [
                (format!("    LFSR 0, 0x{src0:03X}"), loc.clone()),
                (format!("    LFSR 1, 0x{dst0:03X}"), loc.clone()),
                (format!("    MOVLW 0x{n:02X}"), loc.clone()),
                (format!("{l_loop}:"), loc.clone()),
                // POSTINC0 -> POSTINC1: one word pair per byte, both
                // pointers advancing exactly once (the sim resolves the
                // read before the post-increment).
                ("    MOVFF 0xFEE, 0xFE6".to_string(), loc.clone()),
                // The count lives in WREG (file register 0xFE8), which
                // the ISR save area covers, unlike any GPR scratch isel
                // does not own.
                ("    DECFSZ 0xFE8,F,A".to_string(), loc.clone()),
                (format!("    BRA {l_loop}"), loc.clone()),
            ] {
                self.out.push(text);
                self.locs.push(line_loc);
            }
            self.fsr0_holds = None;
            return;
        }
        for (src, dst, loc) in pairs {
            self.out.push(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
            self.locs.push(loc);
        }
    }

    /// Emit a label line and clear the tracked `BSR` and `FSR0` state.
    /// Every label joins paths with different `MOVLB`/FSR0 histories, so
    /// reusing a stale tracked bank or FSR0 position miscompiles: the
    /// reset stays structural here rather than repeated at each call site.
    /// `CALL` returns join the same way but are not labels, so the call
    /// arm clears `self.bsr`/`self.fsr0_holds` directly after emitting `CALL`.
    /// A recorded forward join (`note_branch`) restores agreement: the
    /// fall-through state always joins the recorded branch states, and a
    /// dead fall-through can only add agreement, never break it, so it
    /// joins unconditionally.
    fn emit_label(&mut self, label: &str) {
        let fall = self.bsr;
        self.emit(format!("{label}:"));
        self.bsr = None;
        self.fsr0_holds = None;
        if let Some(agreed) = self.fwd_join.remove(label) {
            // The fall-through state always joins the recorded branch
            // states. A dead fall-through (previous line unconditional)
            // can only add agreement, never break it, so it joins
            // unconditionally rather than detected.
            if let (Some(a), Some(f)) = (agreed, fall) {
                if a == f {
                    self.bsr = Some(a);
                }
            }
        }
    }

    /// Record the tracked bank on a forward edge to a fresh label. Only
    /// forward edges may record: a back-edge lands after its label's
    /// decision, so it must never contribute. Central `emit` calls this
    /// for every single-operand branch to a `tmp` label; block labels
    /// (`{func}_L*`, entries) never match, so IR-pred joins are
    /// unaffected, and loop headers stay unknown because every loop in
    /// this backend enters its header by fall-through or back-edge,
    /// never by a recorded forward branch (audited per loop site).
    fn note_branch(&mut self, label: &str) {
        let cur = self.bsr;
        self.fwd_join
            .entry(label.to_string())
            .and_modify(|e| {
                if *e != cur {
                    *e = None;
                }
            })
            .or_insert(cur);
    }

    /// A branch target that joins through the forward map: a `tmp`
    /// label reached by single-operand branches only. Block labels,
    /// calls, and anything with more than one operand never qualify.
    fn fwd_target(line: &str) -> Option<&str> {
        let mut words = line.split_whitespace();
        let mnem = words.next()?;
        let target = words.next()?;
        if words.next().is_some() {
            return None;
        }
        match mnem {
            "BRA" | "GOTO" | "BZ" | "BNZ" | "BC" | "BNC" | "BN" | "BNN" | "BOV" | "BNOV" => {
                if target.starts_with("tmp") && target[3..].bytes().all(|c| c.is_ascii_digit()) {
                    Some(target)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Emit one `MOVFF src, dst` per pair, in array order. Callers that
    /// need a fixed emission order across aliased memory (e.g. the ISR
    /// epilogue's SFR-vs-retval-backup restores, epic-cc#362) express it
    /// as separate calls rather than one merged array, so the order is a
    /// statement sequence, not a fact buried in array contents.
    fn emit_movff_pairs<const N: usize>(&mut self, pairs: [(u16, u16); N]) {
        for (src, dst) in pairs {
            self.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
        }
    }

    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.tmp);
        *self.tmp += 1;
        s
    }

    fn slot_addr(&self, func: &str, name: &str) -> Slot {
        Slot::Direct(
            *self
                .addrs
                .get(&ssa_key(func, name))
                .unwrap_or_else(|| panic!("isel-pic18: no slot for {func}::{name}")),
        )
    }

    /// The `icmp` a block's terminator can absorb, when that is the
    /// compare's only use in the function.
    ///
    /// Requirements, each of which is load-bearing:
    /// - the terminator is a `BrCond` on a `Reg`;
    /// - the compare is in the *same* block, immediately before the
    ///   terminator. Any instruction between them would be skipped by the
    ///   fused exits, so adjacency is what makes the fusion sound rather
    ///   than merely likely;
    /// - the compare is multi-byte, or a single-byte `eq`/`ne`. Fusing
    ///   skips the 0/1 result byte (preclear, set, reload and test), which
    ///   is an i8 compare's real cost: its own lane is already one word.
    ///   Single-byte ordering compares keep the materializing path, which
    ///   routes them to the per-lane cascade (epic-cc#625).
    ///
    /// A fused compare emits its exits as the branch's own targets and
    /// never writes the result slot, so the 0/1 byte, its preclear, and
    /// the reload (`MOVF`/`BZ`) all disappear.
    fn fusable_icmp<'f>(g: &Gen, f: &'f Func, b: &'f Block) -> Option<&'f Icmp> {
        let [.., Inst::Icmp(c), Inst::BrCond(bc)] = b.insts.as_slice() else {
            return None;
        };
        let Val::Reg(cond) = &bc.cond else {
            return None;
        };
        if c.dst != *cond {
            return None;
        }
        let is_eq_ne = matches!(c.pred.as_str(), "eq" | "ne");
        // The same width guard `emit_inst`'s `Inst::Icmp` arm applies
        // (`n == 1 || n == 2 || n == 4`). Fusing an i64 compare would
        // route it to the chain and compile, while the identical compare
        // with any other use still panics; keep the two paths consistent
        // and the unsupported width loud.
        if !matches!(c.ty.bytes(), 1 | 2 | 4) {
            return None;
        }
        // Only the predicates with a fused lowering. The signed ordering
        // cascades have none (their answer needs the top lane's sign
        // relation), so they keep the materializing path. Single-byte
        // unsigned orderings join them: fusion has no single-lane
        // ordering lowering, only the borrow chain.
        if !matches!(c.pred.as_str(), "eq" | "ne" | "ult" | "uge" | "ugt" | "ule")
            || (c.ty.bytes() == 1 && !is_eq_ne)
        {
            return None;
        }
        // The unsigned ordering compares lower to the borrow chain, which
        // holds STATUS,C across lanes; a rhs lane load that writes C would
        // corrupt it. The materializing path routes that shape to the
        // per-lane cascade instead (see `emit_icmp_i16`), but fusion has
        // no cascade lowering, so decline to fuse it.
        if matches!(c.pred.as_str(), "ult" | "uge" | "ugt" | "ule") && g.load_w_writes_carry(&c.b) {
            return None;
        }
        // The compare's sole consumer must be this branch. Find `b`'s own
        // index so the scan can skip exactly that terminator (the compare
        // itself is a def, not a use, so `read_vals` never counts it).
        let Some(bi_own) = f.blocks.iter().position(|bb| std::ptr::eq(bb, b)) else {
            return None;
        };
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
    fn substitute_asm(&self, template: &str, operands: &[ir::AsmOperand]) -> String {
        for op in operands {
            if let Some(reg) = op.ptr.strip_prefix('%') {
                if let Some((_, k, terms)) = self.resolved.get(&ssa_key(self.cur_func, reg)) {
                    if *k != 0 || !terms.is_empty() {
                        panic!("asm: GEP-derived pointers are not supported; operand {} is derived via getelementptr (only direct locals and globals are allowed)", op.ptr);
                    }
                }
            }
        }
        let mut out = String::with_capacity(template.len() + operands.len() * 6);
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '$' || c == '%' {
                if let Some(&next) = chars.peek() {
                    if next == '%' || next == '$' {
                        chars.next();
                        out.push(next);
                        continue;
                    }
                    if next.is_ascii_digit() {
                        let mut idx_str = String::new();
                        while let Some(&d) = chars.peek() {
                            if d.is_ascii_digit() {
                                idx_str.push(d);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        let idx: usize = idx_str.parse().unwrap();
                        if idx >= operands.len() {
                            panic!("asm: placeholder ${idx} out of range for {} operands in template {template:?}", operands.len());
                        }
                        let ptr = &operands[idx].ptr;
                        let addr = if let Some(g) = ptr.strip_prefix('@') {
                            *self
                                .addrs
                                .get(g)
                                .unwrap_or_else(|| panic!("isel-pic18: no address for @{g}"))
                        } else if let Some(r) = ptr.strip_prefix('%') {
                            self.slot_addr(self.cur_func, r).direct()
                        } else {
                            panic!("asm: malformed operand ptr {ptr:?}");
                        };
                        out.push_str(&format!("0x{addr:02X}"));
                        continue;
                    }
                }
                out.push(c);
            } else {
                out.push(c);
            }
        }
        out
    }
    fn val_addr(&self, v: &Val) -> Slot {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
            Val::Global(g) => Slot::Direct(
                *self
                    .addrs
                    .get(g)
                    .unwrap_or_else(|| panic!("isel-pic18: no address for @{g}")),
            ),
            Val::Const(k) => Slot::Direct((*k & 0xFF) as u16),
        }
    }
    fn global_addr(&self, name: &str) -> u16 {
        *self
            .addrs
            .get(name)
            .unwrap_or_else(|| panic!("isel-pic18: no address for @{name}"))
    }

    /// Whether `name` is a local object in the current function: an
    /// `alloca`'s slot IS the object, so a pointer to it is that slot's
    /// own frame address rather than an address value stored in the slot
    /// (epic-cc#645).
    fn is_local_object(&self, name: &str) -> bool {
        self.m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .map(|f| {
                f.blocks.iter().any(|b| {
                    b.insts
                        .iter()
                        .any(|i| matches!(i, ir::Inst::Alloca(a) if a.dst == name))
                })
            })
            .unwrap_or(false)
    }

    /// Reports whether `name` is a `const` (flash) global: read via `TBLRD`, never
    /// via a RAM address. `alloc` already excludes const globals from the
    /// address map (`const <name>` lines, no address), so this is the only
    /// signal `isel-pic18` needs to route a load to the flash path.
    /// A const that was copied to RAM (alloc placed it in `addrs`) is treated
    /// as RAM.
    fn global_is_const(&self, name: &str) -> bool {
        if self.addrs.contains_key(name) {
            return false;
        }
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.is_const)
            .unwrap_or(false)
    }
    /// Reports whether `name` is a function (a valid indirect-call target) rather
    /// than a RAM/const global. A function's address is a link-time label
    /// literal, materialized as LOW/HIGH bytes, never looked up in the
    /// address map. (epic-cc#73)
    fn is_function(&self, name: &str) -> bool {
        self.m.funcs.iter().any(|f| f.name == name)
    }

    /// The byte width of a value-defining register in the current function
    /// (its slot is `bytes` wide), for folding a 16-bit index register's
    /// high byte into scaled GEP terms (RAM and const-table paths).
    /// Mirrors `alloc::def_width`'s width rules (an `icmp` result is i1
    /// -> 1 byte) so the addition code knows whether to propagate into a
    /// high byte.
    fn reg_width(&self, reg: &str) -> u8 {
        let f = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .unwrap_or_else(|| panic!("isel-pic18: no function {}", self.cur_func));
        for p in &f.params {
            if p.name == reg {
                // sret params are 2-byte address slots; scalar params use
                // their width.
                return if p.sret { 2 } else { p.width };
            }
        }
        for b in &f.blocks {
            for inst in &b.insts {
                let d = match inst {
                    Inst::Load(l) if l.dst == reg => Some(l.ty.bytes()),
                    Inst::Bin(b) if b.dst == reg => Some(b.ty.bytes()),
                    Inst::Zext(z) if z.dst == reg => Some(z.to.bytes()),
                    Inst::Sext(s) if s.dst == reg => Some(s.to.bytes()),
                    Inst::Trunc(t) if t.dst == reg => Some(t.to.bytes()),
                    Inst::IntToPtr(p) if p.dst == reg => Some(p.to.bytes()),
                    Inst::Icmp(c) if c.dst == reg => Some(1),
                    Inst::Select(s) if s.dst == reg => Some(s.ty.bytes()),
                    Inst::Call(c) => match (&c.dst, &c.ty) {
                        (Some(d), Some(t)) if d == reg => Some(t.bytes()),
                        _ => None,
                    },
                    Inst::Phi(p) if p.dst == reg => Some(p.ty.bytes()),
                    Inst::Alloca(a) if a.dst == reg => Some(a.size),
                    Inst::Freeze(f) if f.dst == reg => Some(f.ty.bytes()),
                    // No FloatBin/Fcmp/FloatConv arms: legalize rewrites
                    // every float op before isel runs (a runtime Call, or a
                    // Freeze for fpext/fptrunc), and `emit_inst` panics on
                    // the raw forms, so a width arm here could only be read
                    // for an instruction about to panic. An integer
                    // conversion's width arrives on its call's type (the
                    // `Inst::Call` arm above). (epic-cc#488)
                    Inst::VaArg(v) if v.dst == reg => Some(v.ty.bytes()),
                    _ => None,
                };
                if let Some(w) = d {
                    return w;
                }
            }
        }
        panic!("isel-pic18: no def width for %{reg} in {}", self.cur_func);
    }
    /// Parse a literal-pointer operand (`"0x<K>"`, the `inttoptr` form
    /// irparse produces) into a full 12-bit physical data-space address.
    /// A literal pointer is either an access-bank GPR address (`0x000-
    /// 0x05F`, `a=0`) or an SFR address (`0xF60-0xFFF`, `a=0`): both need
    /// no `BSR` select, which is what makes direct SFR access one
    /// instruction. An address past the 12-bit data space panics.
    fn literal_ptr_addr(&self, ptr: &str) -> u16 {
        let h = ptr
            .strip_prefix("0x")
            .unwrap_or_else(|| panic!("isel-pic18: malformed literal pointer {ptr:?}"));
        let a = u16::from_str_radix(h, 16)
            .unwrap_or_else(|_| panic!("isel-pic18: malformed literal pointer {ptr:?}"));
        assert!(
            a <= 0xFFF,
            "isel-pic18: literal pointer 0x{a:03X} outside the 12-bit data space"
        );
        a
    }

    /// The folded `(base, k, terms)` for pointer reg `r` in the current
    /// function, from the module-wide `resolve_pointers` map. Every `gep`
    /// result and every byval/sret/alloca seed resolves; a plain (not
    /// byval, not sret) pointer PARAMETER never does, since its value is a
    /// runtime address handed in by the caller with no compile-time base
    /// to fold, which `resolve_pointers` has no case for (the pointer lowering scope: see
    /// ADR-009). Panics rather than silently emitting a bogus
    /// access.
    fn resolved_for(&self, r: &str) -> (Base, u16, Vec<(u16, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved.get(&key).cloned().unwrap_or_else(|| {
            panic!(
                "isel-pic18: pointer %{r} ({key}) has no resolved base; only globals, \
                 allocas, byval/sret params, plain pointer params and gep chains off \
                 them resolve (see ADR-009, extended by ADR-018)"
            )
        })
    }

    /// Whether `ptr` is a runtime pointer *value* already sitting, fully
    /// formed, as two bytes in a frame slot -- the common case of
    /// forwarding a plain pointer parameter/local as a call argument (as
    /// opposed to a `gep` off it, or an indexed/computed address). When so,
    /// returns that slot's address: the caller can copy the two bytes
    /// there directly into a callee's param slot instead of routing them
    /// through FSR0 via `emit_ptr_setup` (whose general address-computation
    /// machinery, needed for a real GEP/index, is pure overhead on an
    /// address that is already finished). Mirrors the `Base::Slot`/
    /// `holds_addr` logic in `emit_ptr_setup`'s own resolution, restricted
    /// to the zero-offset, no-dynamic-terms case. (epic-cc#473)
    fn direct_ptr_forward_src(&self, ptr: &Val) -> Option<u16> {
        let Val::Reg(r) = ptr else {
            return None;
        };
        let (base, k, terms) = self.resolved.get(&ssa_key(self.cur_func, r))?;
        if *k != 0 || !terms.is_empty() {
            return None;
        }
        let Base::Slot(sname, indirect) = base else {
            return None;
        };
        let holds_addr = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .map(|f| f.params.iter().any(|p| p.name == *sname && p.ptr))
            .unwrap_or(false);
        (*indirect || holds_addr).then(|| self.slot_addr(self.cur_func, sname).direct())
    }

    /// Reports whether pointer-select dst `name` was seeded by iselcore as an
    /// indirect slot (`Base::Slot(_, true)`): its bytes are a runtime
    /// address VALUE the select must materialize, not a folded pointer.
    fn select_is_seeded(&self, name: &str) -> bool {
        matches!(
            self.resolved.get(&ssa_key(self.cur_func, name)),
            Some((Base::Slot(_, true), 0, t)) if t.is_empty()
        )
    }

    /// Returns the `(a, f)` components for a `W`-routing instruction,
    /// emitting `MOVLB` when the tracked `BSR` mismatches.
    /// Both Access Bank ranges (`0x000-0x05F` and `0xF60-0xFFF`) use `a=0`
    /// with no `MOVLB`; only `0x060-0xF5F` needs a bank select. The SFR
    /// segment holds FSRnL/FSRnH, so hardware requires `a=0` there.
    /// `MOVFF` copies bypass this helper: they carry full 12-bit addresses
    /// and need no bank bit, so `Load`/`Store`/phi/`Call` copies never touch `BSR`.
    fn operand(&mut self, addr: u16) -> (u16, u16) {
        if addr <= self.access_bank_hi || addr >= PIC18_SFR_ACCESS_LO {
            (0, addr & 0xFF)
        } else {
            let bank = (addr >> 8) as u8;
            if self.bsr != Some(bank) {
                self.emit(format!("    MOVLB 0x{bank:X}"));
                self.bsr = Some(bank);
                self.bsr_dirty = true;
            }
            (1, addr & 0xFF)
        }
    }

    /// One byte, memory-to-memory. Staged into `pending_copies` instead
    /// of emitted: consecutive pairs drain as a single POSTINC copy loop
    /// once long enough (epic-cc#486). A pair that breaks consecutivity
    /// drains what is staged first, so the buffer is always one maximal
    /// run. The FSR0-slot invalidation still happens eagerly, at the
    /// copy's logical position, because later address computations read
    /// the tracked state before the drain.
    fn emit_copy_byte(&mut self, src: u16, dst: u16) {
        self.invalidate_fsr0_if_slot_written(dst, 1);
        // A staged copy replays as a raw `MOVFF` push, which the plain
        // `emit` cannot see, so a copy into the tracked slot must drop the
        // belief here: after the drain the byte there is the source's, not
        // W's (epic-cc#502).
        if self.w_holds == Some(dst) {
            self.w_holds = None;
        }
        if let Some(&(last_src, last_dst, _)) = self.pending_copies.last() {
            if src != last_src.wrapping_add(1) || dst != last_dst.wrapping_add(1) {
                self.flush_copies();
            }
        }
        self.pending_copies.push((src, dst, self.cur_loc.clone()));
    }

    /// If `fsr0_holds` is grounded in a `SlotValue` whose slot overlaps
    /// `[dst, dst+nbytes)`, drop the tracked state: this codegen is about
    /// to overwrite the very pointer value a later access's reuse check
    /// would otherwise trust as unchanged (epic-cc#472). A pointer local
    /// that isn't promoted to a pure SSA register (the common case: its
    /// value is reloaded fresh from its slot at every use, exactly the
    /// pattern this reuse optimization targets) is reassigned by an
    /// ordinary write to that same slot, so this is the one place
    /// straight-line code (no label, no `CALL`) can invalidate the
    /// tracked state; `emit_copy_byte` and `emit_move_val_to_slot` are the
    /// two functions every plain "materialize a value into a slot" path
    /// funnels through (direct `Store`, phi-copy resolution, etc.), so
    /// hooking both here covers every write site without threading this
    /// check through each individual caller. A `SlotValue` origin whose
    /// slot does not overlap is untouched; an `Absolute` origin is a
    /// link-time-constant address that no write can ever change, so it is
    /// never invalidated here.
    fn invalidate_fsr0_if_slot_written(&mut self, dst: u16, nbytes: u16) {
        if let Some((Fsr0Origin::SlotValue(slot), _)) = self.fsr0_holds {
            let dst_end = dst + nbytes; // exclusive
            let slot_end = slot + 2; // a pointer value is always 2 bytes
            if dst < slot_end && slot < dst_end {
                self.fsr0_holds = None;
            }
        }
    }

    /// Copy the two-byte ADDRESS VALUE of `val` into the slot at `dst`:
    /// a `Const` literal writes the constant bytes, a `Global` writes its
    /// link-time address as two literals, a `Reg` copies the two bytes of
    /// its runtime-address slot (a seeded select dst, an IntToPtr dst, or
    /// a pointer param). Used by the pointer-select materialization
    /// A reg with dynamic terms is a computed address with
    /// no single materializable value and panics. (epic-cc#147)
    fn emit_move_addr_to_slot(&mut self, val: &Val, dst: u16) {
        match val {
            Val::Const(k) => {
                for (i, byte) in [(0u16, (k & 0xFF) as u8), (1u16, ((k >> 8) & 0xFF) as u8)] {
                    self.emit(format!("    MOVLW 0x{byte:02X}"));
                    let (a, f) = self.operand(dst + i);
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                }
            }
            Val::Global(g) => {
                if self.is_function(g) || self.global_is_const(g) {
                    // A function's address and a flash const table's address
                    // are both link-time label literals (epic-cc#645 for the
                    // const case: `loop-reduce` can turn a const string into
                    // a pointer phi, which then needs the table's flash
                    // address materialized rather than a RAM address).
                    self.emit(format!("    MOVLW LOW({g})"));
                    let (a0, f0) = self.operand(dst);
                    self.emit(format!(
                        "    MOVWF 0x{f0:03X},{}",
                        if a0 == 0 { "A" } else { "B" }
                    ));
                    self.emit(format!("    MOVLW HIGH({g})"));
                    let (a1, f1) = self.operand(dst + 1);
                    self.emit(format!(
                        "    MOVWF 0x{f1:03X},{}",
                        if a1 == 0 { "A" } else { "B" }
                    ));
                } else {
                    let addr = self.global_addr(g);
                    self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                    let (a0, f0) = self.operand(dst);
                    self.emit(format!(
                        "    MOVWF 0x{f0:03X},{}",
                        if a0 == 0 { "A" } else { "B" }
                    ));
                    self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                    let (a1, f1) = self.operand(dst + 1);
                    self.emit(format!(
                        "    MOVWF 0x{f1:03X},{}",
                        if a1 == 0 { "A" } else { "B" }
                    ));
                }
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                assert!(
                    k == 0 && terms.is_empty(),
                    "isel-pic18: cannot materialize a computed address ({base:?} k={k} terms={terms:?}) as a select arm"
                );
                match &base {
                    Base::Slot(sname, true) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        self.emit_copy_byte(sa, dst);
                        self.emit_copy_byte(sa + 1, dst + 1);
                    }
                    Base::Global(name) => {
                        let addr = self.global_addr(name);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        let (a0, f0) = self.operand(dst);
                        self.emit(format!(
                            "    MOVWF 0x{f0:03X},{}",
                            if a0 == 0 { "A" } else { "B" }
                        ));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        let (a1, f1) = self.operand(dst + 1);
                        self.emit(format!(
                            "    MOVWF 0x{f1:03X},{}",
                            if a1 == 0 { "A" } else { "B" }
                        ));
                    }
                    other => panic!("isel-pic18: cannot materialize {other:?} as a select arm"),
                }
            }
        }
    }

    /// Copy `val` (width `ty.bytes`) into the slot starting at `dst`. A
    /// register/global source uses `MOVFF` (no access bit needed); a
    /// constant has no `MOVFF` literal form: a zero byte writes a
    /// one-word `CLRF`, an all-ones byte a one-word `SETF`, and any
    /// other byte stages through `W` via `MOVLW`/`MOVWF` (all forms
    /// touch `operand`/`BSR` the same way: this is the one place a
    /// plain copy still touches `operand`).
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u16) {
        self.invalidate_fsr0_if_slot_written(dst, u16::from(ty.bytes()));
        match val {
            Val::Const(k) => {
                for i in 0..ty.bytes() {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    let (a, f) = self.operand(dst + u16::from(i));
                    let bank = if a == 0 { "A" } else { "B" };
                    if byte == 0 {
                        // A zero byte needs no W staging: CLRF writes it
                        // in one word where the pair costs two. CLRF
                        // sets Z, which is safe here: no lowering reads
                        // STATUS across insts, every consumer sets its
                        // own flags first (compare chains, shift
                        // carries).
                        self.emit(format!("    CLRF 0x{f:03X},{bank}"));
                    } else if byte == 0xFF {
                        // An all-ones byte is the same shape with no
                        // flag hazard at all: SETF touches no STATUS
                        // bit. (epic-cc#666)
                        self.emit(format!("    SETF 0x{f:03X},{bank}"));
                    } else {
                        self.emit(format!("    MOVLW 0x{byte:02X}"));
                        self.emit_w_store(dst + u16::from(i));
                    }
                }
            }
            Val::Reg(r)
                if self
                    .resolved
                    .contains_key(&iselcore::ssa_key(self.cur_func, r)) =>
            {
                let (base, k, terms) = self
                    .resolved
                    .get(&iselcore::ssa_key(self.cur_func, r))
                    .cloned()
                    .unwrap();
                // A global's address is a link-time constant. ipsccp
                // can propagate a global argument into a
                // helper's pointer parameter, so a GEP over it resolves
                // here to `Base::Global`, not a slot. Its base bytes are
                // literals (`MOVLW`), not a `MOVF` read, so it needs its
                // own arm, mirroring `emit_move_addr_to_slot`'s above plus
                // the slot case's k/terms folding. (epic-cc#193)
                if let iselcore::Base::Global(name) = &base {
                    // k and terms compose freely here: the `[]` arm folds
                    // k into the literal, `[(1, reg)]` adds the term
                    // onto it, and anything else seeds FSR0 with the
                    // full address below (epic-cc#610).
                    //
                    // A const (flash) global has no RAM address: its bytes
                    // are a link-time label, so it materializes as
                    // `LOW()/HIGH()` literals the way `emit_move_addr_to_slot`
                    // does (epic-cc#645). A dynamic term cannot ride a
                    // literal, so that combination still needs the FSR0
                    // path below.
                    if self.global_is_const(name) && terms.is_empty() {
                        // `LOW`/`HIGH` are 8-bit link-time literals, so a
                        // constant offset above 255 still needs the carry
                        // out of byte 0 in byte 1: `ADDLW` sets STATUS,C
                        // and the byte-1 pass adds it back, the same idiom
                        // the `[]` arm below uses for a literal base.
                        let adds_in_byte0 = (k & 0xFF) != 0;
                        for i in 0..ty.bytes() {
                            let lit = if i == 0 { "LOW" } else { "HIGH" };
                            self.emit(format!("    MOVLW {lit}({name})"));
                            if i == 0 {
                                let lo = (k & 0xFF) as u8;
                                if lo != 0 {
                                    self.emit(format!("    ADDLW 0x{lo:02X}"));
                                }
                            } else {
                                // Carry-FIRST: byte 0's `ADDLW` set C, and
                                // `MOVLW` leaves it alone, so the low-byte
                                // carry must be consumed before the high
                                // byte's own `ADDLW` overwrites STATUS.C.
                                if adds_in_byte0 {
                                    self.emit("    BTFSC 0xFD8,0,A".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                }
                                let hi = (k >> 8) as u8;
                                if hi != 0 {
                                    self.emit(format!("    ADDLW 0x{hi:02X}"));
                                }
                            }
                            let (a, f) = self.operand(dst + u16::from(i));
                            let bank = if a == 0 { "A" } else { "B" };
                            self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                        }
                        return;
                    }
                    let addr = self.global_addr(name).wrapping_add(k);
                    for i in 0..ty.bytes() {
                        let byte = ((addr >> (i as u32 * 8)) & 0xFF) as u8;
                        match terms.as_slice() {
                            [] => {
                                self.emit(format!("    MOVLW 0x{byte:02X}"));
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                let ra_i = ra + u16::from(i);
                                let (ra_a, ra_f) = self.operand(ra_i);
                                let ra_bank = if ra_a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVLW 0x{byte:02X}"));
                                if i == 0 {
                                    self.emit(format!("    ADDWF 0x{ra_f:03X},W,{ra_bank}"));
                                } else {
                                    self.emit("    BTFSC 0xFD8,0,A".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                    self.emit(format!("    ADDWF 0x{ra_f:03X},W,{ra_bank}"));
                                }
                            }
                            _ => {
                                // A scaled or multi-term index has no ADDWF
                                // form: seed FSR0 with the full address
                                // through the shared load/store scaler
                                // (MULWF or shift-add chain) and read back
                                // FSR0L/H. Immediate emits, never staged
                                // copy_bytes: FSR0 only holds this address
                                // right here. (epic-cc#468)
                                if i == 0 {
                                    self.emit_fsr0_dynamic(self.global_addr(name), k, &terms, 0);
                                }
                                let sfr = if i == 0 { 0xFE9 } else { 0xFEA };
                                let (sa, sf) = self.operand(sfr);
                                self.emit(format!(
                                    "    MOVF 0x{sf:03X},W,{}",
                                    if sa == 0 { "A" } else { "B" }
                                ));
                            }
                        }
                        let (a, f) = self.operand(dst + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                    }
                    return;
                }
                let sa = match &base {
                    iselcore::Base::Slot(sname, indirect) => {
                        // A local object (`alloca`): the slot IS the object,
                        // so the pointer is the slot's own frame address, a
                        // compile-time constant. Fold it as literals rather
                        // than reading the object's first bytes as if they
                        // were an address (epic-cc#645).
                        if !*indirect && self.is_local_object(sname) {
                            // The slot IS the object, so its base is a
                            // compile-time frame address. Fold `k` into it
                            // like the literal-base path below; a dynamic
                            // term cannot ride a literal, so that shape
                            // still needs the FSR path.
                            assert!(
                                terms.is_empty(),
                                "isel-pic18: GEP over a local object with dynamic terms ({terms:?}) is not supported in a move"
                            );
                            let addr = self
                                .slot_addr(self.cur_func, sname)
                                .direct()
                                .wrapping_add(k);
                            for i in 0..ty.bytes() {
                                let byte = ((addr >> (i as u32 * 8)) & 0xFF) as u8;
                                let (aa, af) = self.operand(dst + u16::from(i));
                                let bank = if aa == 0 { "A" } else { "B" };
                                if byte == 0 {
                                    self.emit(format!("    CLRF 0x{af:03X},{bank}"));
                                } else {
                                    self.emit(format!("    MOVLW 0x{byte:02X}"));
                                    self.emit(format!("    MOVWF 0x{af:03X},{bank}"));
                                }
                            }
                            return;
                        }
                        let holds_addr = if *indirect {
                            true
                        } else {
                            self.m
                                .funcs
                                .iter()
                                .find(|f| f.name == self.cur_func)
                                .map(|f| f.params.iter().any(|pp| pp.name == *sname && pp.ptr))
                                .unwrap_or(false)
                        };
                        assert!(
                            holds_addr,
                            "isel-pic18: cannot materialize GEP over {base:?} in move to slot"
                        );
                        self.slot_addr(self.cur_func, sname).direct()
                    }
                    other => {
                        panic!("isel-pic18: cannot materialize GEP over {other:?} in move to slot")
                    }
                };
                let adds_in_byte0 = k != 0 || !terms.is_empty();
                assert!(
                    k == 0 || terms.is_empty(),
                    "isel-pic18: GEP with both k and terms not supported in move"
                );
                for i in 0..ty.bytes() {
                    match terms.as_slice() {
                        [] => {
                            if i == 0 {
                                let (a, f) = self.operand(sa);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                if k != 0 {
                                    self.emit(format!("    ADDLW 0x{k:02X}"));
                                }
                            } else {
                                let (a, f) = self.operand(sa + 1);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                if adds_in_byte0 {
                                    self.emit("    BTFSC 0xFD8,0,A".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                }
                            }
                            let (a, f) = self.operand(dst + u16::from(i));
                            let bank = if a == 0 { "A" } else { "B" };
                            self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                        }
                        [(1, reg)] => {
                            let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                            let (ra_a, ra_f) = self.operand(ra);
                            let ra_bank = if ra_a == 0 { "A" } else { "B" };
                            let ra1 = ra + 1;
                            let (ra1_a, ra1_f) = self.operand(ra1);
                            let ra1_bank = if ra1_a == 0 { "A" } else { "B" };
                            if i == 0 {
                                let (a, f) = self.operand(sa);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                self.emit(format!("    ADDWF 0x{ra_f:03X},W,{ra_bank}"));
                            } else {
                                let (a, f) = self.operand(sa + 1);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                self.emit("    BTFSC 0xFD8,0,A".to_string());
                                self.emit("    ADDLW 0x01".to_string());
                                self.emit(format!("    ADDWF 0x{ra1_f:03X},W,{ra1_bank}"));
                            }
                            let (a, f) = self.operand(dst + u16::from(i));
                            let bank = if a == 0 { "A" } else { "B" };
                            self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                        }
                        _ => panic!("isel-pic18: multi-term GEP move with {terms:?} not supported"),
                    }
                }
            }
            Val::Global(g) if self.is_function(g) => {
                // A function's address is a link-time label literal: byte 0
                // = LOW(g), byte 1 = HIGH(g). (epic-cc#73)
                for i in 0..ty.bytes() {
                    let lit = if i == 0 { "LOW" } else { "HIGH" };
                    self.emit(format!("    MOVLW {lit}({g})"));
                    let (a, f) = self.operand(dst + u16::from(i));
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                }
            }
            Val::Global(g) => {
                // A data global in value position is a pointer ADDRESS (a
                // `store ptr @g, ...` or a pointer phi incoming; clang
                // always loads scalar globals first): materialize it as two
                // literals, never copy the pointee's contents. (epic-cc#155)
                // A const (flash) global has no RAM address: its table
                // label is the link-time literal, like a function above.
                if self.global_is_const(g) {
                    for i in 0..ty.bytes() {
                        let lit = if i == 0 { "LOW" } else { "HIGH" };
                        self.emit(format!("    MOVLW {lit}({g})"));
                        let (a, f) = self.operand(dst + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                    }
                    return;
                }
                let addr = self.global_addr(g);
                for i in 0..ty.bytes() {
                    let byte = ((addr >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVLW 0x{byte:02X}"));
                    let (a, f) = self.operand(dst + u16::from(i));
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                }
            }
            _ => {
                let src = self.val_addr(val).direct();
                for i in 0..ty.bytes() {
                    self.emit_copy_byte_or_w(src + u16::from(i), dst + u16::from(i));
                }
            }
        }
    }

    /// How a byte access at `ptr + byte_off` completes. `Val::Global`
    /// resolves directly (globals are never behind another indirection
    /// layer). `Val::Reg` looks up the pointer's fold via `resolved_for`:
    /// a `Base::Global`/`Base::Slot(_, false)` (byval/alloca, the slot
    /// itself IS the object) with no dynamic terms is a plain direct
    /// address; with dynamic terms it needs FSR0 (the dynamic-pointer setup). A
    /// `Base::Slot(_, true)` (sret, the slot holds a target ADDRESS, not
    /// the object) always needs FSR0, regardless of terms (the sret-address setup).
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => Addr::Direct(self.global_addr(g).wrapping_add(u16::from(byte_off))),
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        if terms.is_empty() {
                            Addr::Direct(
                                self.global_addr(name)
                                    .wrapping_add(k)
                                    .wrapping_add(u16::from(byte_off)),
                            )
                        } else {
                            self.emit_fsr0_dynamic(self.global_addr(name), k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        // A plain pointer param's slot holds the address rather
                        // than being the object, so it is read like an `sret`
                        // slot (a `byval` param's slot IS the object).
                        let holds_addr = self
                            .m
                            .funcs
                            .iter()
                            .find(|f| f.name == self.cur_func)
                            .map(|f| f.params.iter().any(|p| p.name == *sname && p.ptr))
                            .unwrap_or(false);
                        if *indirect || holds_addr {
                            self.emit_fsr0_indirect_slot(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else if terms.is_empty() {
                            Addr::Direct(sa.wrapping_add(k).wrapping_add(u16::from(byte_off)))
                        } else {
                            self.emit_fsr0_dynamic(sa, k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel-pic18: pointer operand must be a register or global"),
        }
    }

    /// Whether `ptr` is a byte-indexed access into a small RAM global:
    /// `Base::Global` with exactly one scale-1 term. The global must hold
    /// 1 to 128 bytes: a valid index stays below 128, where the `PLUSW0`
    /// offset (signed `W`, like the simulator models it) agrees with the
    /// unsigned address arithmetic it replaces. Out-of-bounds indices are
    /// C UB, so the valid domain is the whole contract. Const (flash)
    /// globals never qualify: they read through `TBLRD` on an earlier
    /// arm. Frame arrays and multi-term/scaled accesses keep the `INDF0`
    /// lowering. (epic-cc#665)
    fn plusw_shape(&self, ptr_val: &Val) -> Option<PluswShape> {
        let Val::Reg(r) = ptr_val else {
            return None;
        };
        let (base, k, terms) = self.resolved_for(r);
        let Base::Global(name) = &base else {
            return None;
        };
        if self.global_is_const(name) {
            return None;
        }
        if terms.len() != 1 || terms[0].0 != 1 {
            return None;
        }
        let size = self.m.globals.iter().find(|g| &g.name == name)?.size;
        if size == 0 || size > 128 {
            return None;
        }
        // The index must be a plain data value with a slot. Address-valued
        // regs (a stored GEP, epic-cc#468) materialize through `FSR0`,
        // not through a slot, so there is nothing to load `W` from.
        let idx_slot = *self.addrs.get(&ssa_key(self.cur_func, &terms[0].1))?;
        Some(PluswShape {
            idx_slot,
            base: self.global_addr(name),
            k,
        })
    }

    /// Seed `FSR0` with the access's static part (reusing a resident
    /// pointer through `try_reuse_fsr0`, which is sound here because a
    /// `PLUSW0` access never moves `FSR0`) and load the byte index into
    /// `W`, last so a reuse delta's `MOVLW` cannot clobber it. The caller
    /// completes the access through `PLUSW0` (0xFEB) in the same straight
    /// line: `FSR0`, `W`, and the index slot must not be touched between
    /// this setup and that access. (epic-cc#665)
    fn emit_plusw_setup(&mut self, shape: &PluswShape, byte_off: u8) {
        self.flush_copies();
        let static_part = shape.k.wrapping_add(u16::from(byte_off));
        let origin = Fsr0Origin::Absolute(shape.base);
        if !self.try_reuse_fsr0(origin, static_part) {
            let lit = shape.base.wrapping_add(static_part) & 0xFFF;
            self.emit(format!("    LFSR 0, 0x{lit:03X}"));
        }
        self.fsr0_holds = Some((origin, static_part));
        let (ia, iff) = self.operand(shape.idx_slot);
        self.emit(format!(
            "    MOVF 0x{iff:03X},W,{}",
            if ia == 0 { "A" } else { "B" }
        ));
    }

    /// Set up the memcpy SOURCE pointer on FSR1 (an indirect source would
    /// otherwise be clobbered by the destination's FSR0 setup) and return
    /// `Some(direct_addr)` for a direct source, `None` for an indirect one
    /// (the byte is read via INDF1, 0xFE7). Mirrors `emit_ptr_setup`'s
    /// resolution, targeting FSR1L/FSR1H (0xFE1/0xFE2).
    fn emit_memcpy_src_setup(&mut self, ptr: &Val, byte_off: u8) -> Option<u16> {
        match ptr {
            Val::Global(g) => Some(self.global_addr(g) + u16::from(byte_off)),
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        if terms.is_empty() {
                            Some(
                                self.global_addr(name)
                                    .wrapping_add(k)
                                    .wrapping_add(u16::from(byte_off)),
                            )
                        } else {
                            self.emit_fsr1_dynamic(self.global_addr(name), k, &terms, byte_off);
                            None
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        let holds_addr = self
                            .m
                            .funcs
                            .iter()
                            .find(|f| f.name == self.cur_func)
                            .map(|f| f.params.iter().any(|p| p.name == *sname && p.ptr))
                            .unwrap_or(false);
                        if *indirect || holds_addr {
                            self.emit_fsr1_indirect_slot(sa, k, &terms, byte_off);
                            None
                        } else if terms.is_empty() {
                            // `k` can arrive wrapped from a negative GEP
                            // (a loop-reduce back edge), so the sum is
                            // modular like every sibling arm (epic-cc#645).
                            Some(sa.wrapping_add(k).wrapping_add(u16::from(byte_off)))
                        } else {
                            self.emit_fsr1_dynamic(sa, k, &terms, byte_off);
                            None
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel-pic18: pointer operand must be a register or global"),
        }
    }

    /// Set `FSR1 = base_addr + k + Σ scale×%reg + byte_off` and leave the
    /// access to go through `INDF1` (0xFE7). FSR1 mirror of
    /// `emit_fsr0_dynamic` (LFSR 1, FSR1L/FSR1H = 0xFE1/0xFE2).
    fn emit_fsr1_dynamic(&mut self, base_addr: u16, k: u16, terms: &[(u16, String)], byte_off: u8) {
        let static_part = k.wrapping_add(u16::from(byte_off));
        let lit = base_addr.wrapping_add(static_part) & 0xFFF;
        let chain = self.chain_term_index(terms, 0);
        let Some((ci, form)) = chain else {
            self.emit(format!("    LFSR 1, 0x{lit:03X}"));
            self.add_term_to_fsr1(terms);
            return;
        };
        // Same seed-first shape as `emit_fsr0_dynamic`: MULWF loads the
        // static base then adds the scaled term onto it, the chain
        // zero-seeds and re-adds the base as a literal afterwards.
        let (scale, reg) = &terms[ci];
        let a = self.slot_addr(self.cur_func, reg).direct();
        let wide = self.reg_width(reg) == 2;
        match form {
            Scaled::Mulwf => {
                self.emit(format!("    LFSR 1, 0x{lit:03X}"));
                self.emit_mulwf_scale(0xFE1, 0xFE2, *scale, a, wide);
            }
            Scaled::Chain => {
                self.emit("    LFSR 1, 0x000".to_string());
                self.emit_scale_chain(0xFE1, 0xFE2, *scale, a, wide);
                self.emit_fsr_pair_add_lit(0xFE1, 0xFE2, base_addr.wrapping_add(static_part));
            }
        }
        self.add_terms_except(terms, Some(ci), 0xFE1, 0xFE2);
    }
    /// `slot_addr` holds a 2-byte ADDRESS (an sret param's contents), not
    /// the object itself: load THAT address into FSR1, then add the static
    /// offset and any dynamic terms. FSR1 mirror of `emit_fsr0_indirect_slot`.
    fn emit_fsr1_indirect_slot(
        &mut self,
        slot_addr: u16,
        k: u16,
        terms: &[(u16, String)],
        byte_off: u8,
    ) {
        let static_part = k.wrapping_add(u16::from(byte_off));
        // Same accounting as `emit_fsr0_indirect_slot`: CLRF pair plus a
        // runtime re-add of the pointer's own value.
        let extra = 2 + if static_part != 0 { 4 } else { 0 };
        let chain = self.chain_term_index(terms, extra);
        if let Some((ci, form)) = chain {
            let (scale, reg) = &terms[ci];
            let a = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            match form {
                Scaled::Mulwf => {
                    // Seed the pointer's own value, then add the scaled
                    // term onto it. No zero-seed dance is needed, so the
                    // static part keeps the two-byte load form.
                    self.emit_copy_byte(slot_addr, 0xFE1); // FSR1L
                    self.emit_copy_byte(slot_addr + 1, 0xFE2); // FSR1H
                    if static_part != 0 {
                        self.emit_fsr_pair_add_lit(0xFE1, 0xFE2, u16::from(static_part));
                    }
                    self.emit_mulwf_scale(0xFE1, 0xFE2, *scale, a, wide);
                }
                Scaled::Chain => {
                    self.emit("    CLRF 0x0E1,A".to_string()); // FSR1L = 0
                    self.emit("    CLRF 0x0E2,A".to_string()); // FSR1H = 0
                    self.emit_scale_chain(0xFE1, 0xFE2, *scale, a, wide);
                    self.emit_fsr_pair_add_mem16(0xFE1, 0xFE2, slot_addr, slot_addr + 1);
                    if static_part != 0 {
                        self.emit_fsr_pair_add_lit(0xFE1, 0xFE2, u16::from(static_part));
                    }
                }
            }
            self.add_terms_except(terms, Some(ci), 0xFE1, 0xFE2);
            return;
        }
        self.emit_copy_byte(slot_addr, 0xFE1); // FSR1L = low byte of the stored address
        self.emit_copy_byte(slot_addr + 1, 0xFE2); // FSR1H = high byte
        if static_part != 0 {
            self.emit_fsr_pair_add_lit(0xFE1, 0xFE2, u16::from(static_part));
        }
        self.add_term_to_fsr1(terms);
    }
    /// The naive accumulation loop for `FSR1`, mirroring
    /// `add_term_to_fsr0`: small scales and residual terms after a
    /// shift-add chain, with a 16-bit index's high byte folded into
    /// every repetition.
    fn add_term_to_fsr1(&mut self, terms: &[(u16, String)]) {
        for (scale, reg) in terms {
            let a = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            for _ in 0..*scale {
                self.emit_fsr_pair_add(0xFE1, 0xFE2, a, wide);
            }
        }
    }

    /// Memcpy setups only, for the byte at `byte_off`: seed the source
    /// (`TBLPTR` or FSR1) then the destination (FSR0), in that order,
    /// reporting the resolved kinds. Bodies live in `emit_memcpy_body`
    /// so one seed can serve a whole walk. (epic-cc#492)
    fn emit_memcpy_setups(&mut self, mc: &ir::Memcpy, byte_off: u8) -> (McSrc, Addr) {
        // Sets the source on FSR1 so the destination FSR0 setup cannot clobber
        // an indirect source (epic-cc#143); a flash source has no RAM
        // address and seeds `TBLPTR` instead.
        if let Some((table, k, terms)) = self.const_base_of(&mc.src) {
            self.emit_tblptr_setup(&table, k, &terms, byte_off);
            return (McSrc::Tblrd, self.emit_ptr_setup(&mc.dst, byte_off));
        }
        let src_direct = self.emit_memcpy_src_setup(&mc.src, byte_off);
        let dst = self.emit_ptr_setup(&mc.dst, byte_off);
        (src_direct.map_or(McSrc::Indirect, McSrc::Direct), dst)
    }

    /// Whether a const-source copy of `n` bytes should run as one loop
    /// rather than `n` unrolled `TBLRD`+`MOVFF` pairs: the table costs
    /// `ceil(n/2)` flash words of its own plus a 14-word loop, against the
    /// unrolled form's 3 words per byte (epic-cc#486's loop shape with
    /// `TBLRD*+` as the reader). Reuses the same floor as the RAM copy
    /// loop so both copy families switch at one place.
    fn const_copy_loops(&self, n: u8) -> bool {
        usize::from(n) >= COPY_LOOP_MIN_PAIRS
    }

    /// `n` bytes from the const table `TBLPTR` already points at into a
    /// direct destination `dst`, as one counted loop. `TBLRD*+` advances
    /// the table pointer, the count lives in WREG (covered by the ISR save
    /// area), and the body writes through POSTINC1 so one `LFSR` covers
    /// the whole destination (epic-cc#504).
    fn emit_const_copy_loop(&mut self, dst: u16, n: u8) {
        let l_loop = self.fresh_label();
        self.emit(format!("    LFSR 1, 0x{dst:03X}"));
        self.emit(format!("    MOVLW 0x{n:02X}"));
        self.emit_label(&l_loop);
        self.emit("    TBLRD*+".to_string());
        // TABLAT -> POSTINC1: one word pair per byte, the destination
        // pointer advancing once (the sim resolves the read first).
        self.emit("    MOVFF 0xFF5, 0xFE6".to_string());
        self.emit("    DECFSZ 0xFE8,F,A".to_string());
        self.emit(format!("    BRA {l_loop}"));
        // FSR1 is dead after the walk (nothing reuses it), but FSR0 is not
        // touched, so the tracked state survives.
    }

    /// One memcpy body for already-resolved (`src`, `dst`) positions.
    /// With `walk`, every walked pointer advances on every byte;
    /// without it every byte reads INDF, the pre-walk form. Dynamic
    /// terms fold into the byte-0 seed once (they must be loop-invariant
    /// across the copy, the `Load`/`Store` contract). An indirect
    /// destination closes with INDF on the last byte, leaving FSR0 on
    /// it for `bump_fsr0_tracked_offset`; direct addresses arrive fully
    /// resolved. (epic-cc#492)
    fn emit_memcpy_body(&mut self, last: bool, src: &McSrc, dst: &Addr, walk: bool) {
        match (*src, *dst) {
            (McSrc::Direct(a), Addr::Direct(d)) => {
                self.emit_copy_byte(a, d);
            }
            (McSrc::Indirect, Addr::Direct(d)) => {
                // The byte moves through W, which the ISR saves, so an
                // interrupt mid-copy keeps the held byte. (epic-cc#143)
                let f = if walk { "0xFE6" } else { "0xFE7" }; // POSTINC1 : INDF1
                self.emit(format!("    MOVF {f},W,A"));
                self.emit_banked("MOVWF", d, "");
            }
            (McSrc::Direct(a), Addr::Indirect) => {
                let d = if walk && !last { "0xFEE" } else { "0xFEF" }; // POSTINC0 : INDF0
                self.emit(format!("    MOVFF 0x{a:03X}, {d}"));
            }
            (McSrc::Indirect, Addr::Indirect) => {
                let (s, d) = if walk {
                    ("0xFE6", if !last { "0xFEE" } else { "0xFEF" })
                } else {
                    ("0xFE7", "0xFEF")
                };
                self.emit(format!("    MOVFF {s}, {d}"));
            }
            (McSrc::Tblrd, Addr::Direct(d)) => {
                self.emit(if walk {
                    "    TBLRD*+".to_string()
                } else {
                    "    TBLRD*".to_string()
                });
                self.emit(format!("    MOVFF 0xFF5, 0x{d:03X}"));
            }
            (McSrc::Tblrd, Addr::Indirect) => {
                self.emit(if walk {
                    "    TBLRD*+".to_string()
                } else {
                    "    TBLRD*".to_string()
                });
                let d = if walk && !last { "0xFEE" } else { "0xFEF" }; // POSTINC0 : INDF0
                self.emit(format!("    MOVFF 0xFF5, {d}"));
            }
        }
    }

    /// Try to reuse FSR0's currently tracked contents for a new access
    /// grounded in `origin` at offset `target_off`, emitting only the
    /// forward delta instead of a full base-plus-offset setup. Returns
    /// `true` when it reused (the caller's full setup is skipped
    /// entirely); `false` when the tracked state doesn't match (unknown,
    /// a different origin, or `target_off` is behind the tracked
    /// position). A `false` caller performs its normal full setup and
    /// is responsible for recording the new position itself. (epic-cc#472)
    fn try_reuse_fsr0(&mut self, origin: Fsr0Origin, target_off: u16) -> bool {
        let Some((cur_origin, cur_off)) = self.fsr0_holds else {
            return false;
        };
        if cur_origin != origin || target_off < cur_off {
            return false;
        }
        let delta = target_off - cur_off;
        // A nonzero delta costs `MOVLW`/`ADDWF`/`MOVLW`/`ADDWFC` (4
        // words) against a fresh `LFSR` (2), so an absolute base never
        // takes it. A slot-held pointer has no literal to seed from,
        // so only it walks forward. (epic-cc#665)
        if delta != 0 && matches!(origin, Fsr0Origin::Absolute(_)) {
            return false;
        }
        if delta != 0 {
            self.emit(format!("    MOVLW 0x{:02X}", (delta & 0xFF) as u8));
            let (fa, ff) = self.operand(0xFE9);
            self.emit(format!(
                "    ADDWF 0x{ff:03X},F,{}",
                if fa == 0 { "A" } else { "B" }
            ));
            self.emit(format!("    MOVLW 0x{:02X}", (delta >> 8) as u8));
            let (ha, hf) = self.operand(0xFEA);
            self.emit(format!(
                "    ADDWFC 0x{hf:03X},F,{}",
                if ha == 0 { "A" } else { "B" }
            ));
        }
        true
    }

    /// After a multi-byte indirect `Load`/`Store` walks `n` bytes via
    /// `POSTINC0` (epic-cc#471), FSR0 physically ends up `n - 1` past
    /// where `emit_ptr_setup` left it recorded (the setup records the
    /// *first* byte's offset; the loop's own `POSTINC0` steps advance
    /// past every byte but the last, which stays on `INDF0`). Reflect
    /// that in `fsr0_holds` so the next access's delta is computed from
    /// where FSR0 truly is. A no-op when `fsr0_holds` is `None` (the
    /// access had dynamic terms, so nothing was tracked to begin with).
    fn bump_fsr0_tracked_offset(&mut self, n: u8) {
        if let Some((origin, off)) = self.fsr0_holds {
            // Saturating: `off` is a byte offset within the origin's
            // object and `n` a multi-byte access width, so the sum can
            // exceed `u16` on a high frame. `try_reuse_fsr0` only compares
            // this and emits the difference, so a saturated value can only
            // refuse a reuse, never mis-offset an access.
            self.fsr0_holds = Some((origin, off.saturating_add(u16::from(n) - 1)));
        }
    }

    /// Sets `FSR0 = base_addr + k + Σ terms + byte_off` for `INDF0` access.
    /// `LFSR` seeds the static part in one instruction; every dynamic term
    /// folds in via `ADDWF`/`ADDWFC` through `operand()`, which treats
    /// FSR0L/FSR0H as the always-access-bank SFR segment, so no `MOVLB`
    /// emits. See ADR-009 item 6 and its follow-up note on multi-term
    /// support.
    ///
    /// A big-enough term switches to a shift-add chain from a zero seed,
    /// with the static part re-joining as a literal add afterwards.
    ///
    /// When `terms` is empty and FSR0 already holds this same `base_addr`
    /// at or before the target offset, `try_reuse_fsr0` emits the forward
    /// delta instead of the full `LFSR` (epic-cc#472); a non-empty `terms`
    /// always takes the full setup, since the reuse check has no way to
    /// represent a previously-added dynamic term's runtime contribution.
    fn emit_fsr0_dynamic(&mut self, base_addr: u16, k: u16, terms: &[(u16, String)], byte_off: u8) {
        // A staged run may drain here as a POSTINC loop that moves FSR0
        // wholesale; the tracked position is only sound after the drain,
        // so every reuse decision below sees post-drain state.
        self.flush_copies();
        let static_part = k.wrapping_add(u16::from(byte_off));
        let origin = Fsr0Origin::Absolute(base_addr);
        let chain = self.chain_term_index(terms, 0);
        let Some((ci, form)) = chain else {
            if !(terms.is_empty() && self.try_reuse_fsr0(origin, static_part)) {
                let lit = base_addr.wrapping_add(static_part) & 0xFFF;
                self.emit(format!("    LFSR 0, 0x{lit:03X}"));
                self.add_term_to_fsr0(terms);
            }
            self.fsr0_holds = terms.is_empty().then_some((origin, static_part));
            return;
        };
        // The doubling stage needs a zero-seeded FSR0, so the static
        // base cannot ride the LFSR literal; it re-joins as one 16-bit
        // literal add after the chain, still far cheaper than the naive
        // loop it replaces once the stride is big enough. MULWF instead
        // seeds the base first and adds the scaled term onto it.
        let (scale, reg) = &terms[ci];
        let a = self.slot_addr(self.cur_func, reg).direct();
        let wide = self.reg_width(reg) == 2;
        match form {
            Scaled::Mulwf => {
                let lit = base_addr.wrapping_add(static_part) & 0xFFF;
                self.emit(format!("    LFSR 0, 0x{lit:03X}"));
                self.emit_mulwf_scale(0xFE9, 0xFEA, *scale, a, wide);
            }
            Scaled::Chain => {
                self.emit("    LFSR 0, 0x000".to_string());
                self.emit_scale_chain(0xFE9, 0xFEA, *scale, a, wide);
                self.emit_fsr_pair_add_lit(0xFE9, 0xFEA, base_addr.wrapping_add(static_part));
            }
        }
        self.add_terms_except(terms, Some(ci), 0xFE9, 0xFEA);
        // The tracker cannot represent a runtime term's contribution.
        self.fsr0_holds = None;
    }
    /// `slot_addr` holds a 2-byte ADDRESS (an sret param's contents), not
    /// the object itself: load THAT address into FSR0 (`MOVFF slot,
    /// FSR0L` / `MOVFF slot+1, FSR0H`, both plain memory-to-memory, no
    /// access bit needed since MOVFF never uses one), then add the static
    /// offset (`k + byte_off`) and every dynamic term the same way
    /// `emit_fsr0_dynamic` does, and access through `INDF0`.
    ///
    /// Same `try_reuse_fsr0` shortcut as `emit_fsr0_dynamic`: when `terms`
    /// is empty and FSR0 already holds a value loaded from this same
    /// `slot_addr` (and nothing has written to that slot since, tracked by
    /// `invalidate_fsr0_if_slot_written`), skip the base reload and add
    /// only the forward delta (epic-cc#472).
    fn emit_fsr0_indirect_slot(
        &mut self,
        slot_addr: u16,
        k: u16,
        terms: &[(u16, String)],
        byte_off: u8,
    ) {
        // Same drain-before-decision as `emit_fsr0_dynamic`: a pending
        // run draining as a POSTINC loop moves FSR0, so the reuse check
        // must run against post-drain state.
        self.flush_copies();
        let origin = Fsr0Origin::SlotValue(slot_addr);
        let static_part = k.wrapping_add(u16::from(byte_off));
        // Chain overhead: two CLRFs plus a runtime re-add of the pointer's
        // own value (the chain's zero seed can't carry it, unlike
        // `emit_fsr0_dynamic`'s compile-time `base_addr`) replace the
        // freshly loaded address pair, plus the static re-add when one
        // exists.
        let extra = 2 + if static_part != 0 { 4 } else { 0 };
        let chain = self.chain_term_index(terms, extra);
        if let Some((ci, form)) = chain {
            let (scale, reg) = &terms[ci];
            let a = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            match form {
                Scaled::Mulwf => {
                    // Seed the pointer's own value, then add the scaled
                    // term onto it (no zero-seed needed).
                    self.emit_copy_byte(slot_addr, 0xFE9); // FSR0L
                    self.emit_copy_byte(slot_addr + 1, 0xFEA); // FSR0H
                    if static_part != 0 {
                        self.emit_fsr_pair_add_lit(0xFE9, 0xFEA, u16::from(static_part));
                    }
                    self.emit_mulwf_scale(0xFE9, 0xFEA, *scale, a, wide);
                }
                Scaled::Chain => {
                    self.emit("    CLRF 0x0E9,A".to_string()); // FSR0L = 0
                    self.emit("    CLRF 0x0EA,A".to_string()); // FSR0H = 0
                    self.emit_scale_chain(0xFE9, 0xFEA, *scale, a, wide);
                    // The chain only holds scale*idx; fold in the pointer's
                    // own runtime value, which the zero seed couldn't carry.
                    self.emit_fsr_pair_add_mem16(0xFE9, 0xFEA, slot_addr, slot_addr + 1);
                    if static_part != 0 {
                        self.emit_fsr_pair_add_lit(0xFE9, 0xFEA, u16::from(static_part));
                    }
                }
            }
            self.add_terms_except(terms, Some(ci), 0xFE9, 0xFEA);
            // The tracker cannot represent a runtime term's contribution.
            self.fsr0_holds = None;
            return;
        }
        if !(terms.is_empty() && self.try_reuse_fsr0(origin, static_part)) {
            self.emit_copy_byte(slot_addr, 0xFE9); // FSR0L = low byte of the stored address
            self.emit_copy_byte(slot_addr + 1, 0xFEA); // FSR0H = high byte
            if static_part != 0 {
                self.emit_fsr_pair_add_lit(0xFE9, 0xFEA, u16::from(static_part));
            }
            self.add_term_to_fsr0(terms);
        }
        self.fsr0_holds = terms.is_empty().then_some((origin, static_part));
    }
    /// Add every dynamic term onto `FSR0L`/`FSR0H` with carry, `scale`
    /// times each per term: the naive path for small scales and residual
    /// terms after a chain. Each 4-instruction sequence is a
    /// self-contained add-with-carry against the running FSR0 value (the
    /// `ADDWF` sets carry, the following `ADDWFC` consumes it), so
    /// multiple terms accumulate correctly in any order. A 16-bit index
    /// register folds its high byte into every repetition.
    fn add_term_to_fsr0(&mut self, terms: &[(u16, String)]) {
        for (scale, reg) in terms {
            let a = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            for _ in 0..*scale {
                self.emit_fsr_pair_add(0xFE9, 0xFEA, a, wide);
            }
        }
    }

    /// One 4-word add of a dynamic index onto the SFR pair at (`lo`,
    /// `hi`), both always-access-bank: `MOVF idx,W; ADDWF lo,F` seeds the
    /// carry, and the high add consumes either the index's real second
    /// byte (`wide`, `MOVF idx_hi,W; ADDWFC hi,F`) or a zero (`MOVLW 0`,
    /// same word cost). Neither `MOVF` nor `MOVLW` touches C, so the
    /// `ADDWF`-sets-carry / `ADDWFC`-consumes discipline lets sequences
    /// compose in any order.
    fn emit_fsr_pair_add(&mut self, lo: u16, hi: u16, idx_addr: u16, wide: bool) {
        let (ra, rf) = self.operand(idx_addr);
        self.emit(format!(
            "    MOVF 0x{rf:03X},W,{}",
            if ra == 0 { "A" } else { "B" }
        ));
        let (fa, ff) = self.operand(lo);
        self.emit(format!(
            "    ADDWF 0x{ff:03X},F,{}",
            if fa == 0 { "A" } else { "B" }
        ));
        if wide {
            let (wa, wf) = self.operand(idx_addr + 1);
            self.emit(format!(
                "    MOVF 0x{wf:03X},W,{}",
                if wa == 0 { "A" } else { "B" }
            ));
        } else {
            self.emit("    MOVLW 0x00".to_string());
        }
        let (ha, hf) = self.operand(hi);
        self.emit(format!(
            "    ADDWFC 0x{hf:03X},F,{}",
            if ha == 0 { "A" } else { "B" }
        ));
    }

    /// Add the 16-bit value stored at (`mem_lo`, `mem_hi`) onto the SFR
    /// pair at (`lo`, `hi`), with carry (`MOVF mem_lo,W; ADDWF lo,F; MOVF
    /// mem_hi,W; ADDWFC hi,F`). Used to fold a runtime pointer's own
    /// value into a chain-scaled FSR pair: the chain's zero seed can only
    /// hold the running `scale*idx` product, so a `SlotValue` origin's
    /// base (unlike `Absolute`'s compile-time `base_addr`) must be added
    /// as a memory operand after the chain, not folded into the seed.
    fn emit_fsr_pair_add_mem16(&mut self, lo: u16, hi: u16, mem_lo: u16, mem_hi: u16) {
        let (ma, mf) = self.operand(mem_lo);
        self.emit(format!(
            "    MOVF 0x{mf:03X},W,{}",
            if ma == 0 { "A" } else { "B" }
        ));
        let (fa, ff) = self.operand(lo);
        self.emit(format!(
            "    ADDWF 0x{ff:03X},F,{}",
            if fa == 0 { "A" } else { "B" }
        ));
        let (ma2, mf2) = self.operand(mem_hi);
        self.emit(format!(
            "    MOVF 0x{mf2:03X},W,{}",
            if ma2 == 0 { "A" } else { "B" }
        ));
        let (ha, hf) = self.operand(hi);
        self.emit(format!(
            "    ADDWFC 0x{hf:03X},F,{}",
            if ha == 0 { "A" } else { "B" }
        ));
    }

    /// Add the 16-bit literal `v` onto the SFR pair at (`lo`, `hi`).
    fn emit_fsr_pair_add_lit(&mut self, lo: u16, hi: u16, v: u16) {
        self.emit(format!("    MOVLW 0x{:02X}", v & 0xFF));
        let (fa, ff) = self.operand(lo);
        self.emit(format!(
            "    ADDWF 0x{ff:03X},F,{}",
            if fa == 0 { "A" } else { "B" }
        ));
        self.emit(format!("    MOVLW 0x{:02X}", v >> 8));
        let (ha, hf) = self.operand(hi);
        self.emit(format!(
            "    ADDWFC 0x{hf:03X},F,{}",
            if ha == 0 { "A" } else { "B" }
        ));
    }

    /// Compute `scale * idx` into the zeroed FSR pair at (`lo`, `hi`):
    /// one index add, then per lower bit of `scale` a carry cleared
    /// shift left and a conditional index add. Doubling from a zero
    /// seed is what lets the pair hold the running product, so the
    /// static base cannot ride the seed and re-joins as a literal add
    /// afterwards. A 16-bit index (`wide`) rides the same chain: its
    /// high byte replaces the carry-fill zero in every index add, at
    /// the identical word cost.
    fn emit_scale_chain(&mut self, lo: u16, hi: u16, scale: u16, idx_addr: u16, wide: bool) {
        self.emit_fsr_pair_add(lo, hi, idx_addr, wide);
        let bits = 16 - scale.leading_zeros();
        for i in (0..bits - 1).rev() {
            self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS,C = 0
            let (la, lf) = self.operand(lo);
            self.emit(format!(
                "    RLCF 0x{lf:03X},F,{}",
                if la == 0 { "A" } else { "B" }
            ));
            let (ha, hf) = self.operand(hi);
            self.emit(format!(
                "    RLCF 0x{hf:03X},F,{}",
                if ha == 0 { "A" } else { "B" }
            ));
            if (scale >> i) & 1 == 1 {
                self.emit_fsr_pair_add(lo, hi, idx_addr, wide);
            }
        }
    }

    /// Word cost of `emit_scale_chain` for `scale`.
    fn scale_chain_words(scale: u16) -> u16 {
        let bits = 16 - scale.leading_zeros() as u16;
        4 + (bits - 1) * 3 + (scale.count_ones() as u16 - 1) * 4
    }

    /// `FSR pair += scale * idx` through the hardware multiplier, the
    /// cheapest scaled term on any PIC18 with `MULWF` (epic-cc#477):
    /// `MOVLW scale; MULWF idx; MOVF PRODL,W; ADDWF lo,F; MOVF PRODH,W;
    /// ADDWFC hi,F`, 6 words flat. A 16-bit index adds its own scaled
    /// high byte (`MOVLW scale; MULWF idx_hi; MOVF PRODL,W; ADDWF hi,F`),
    /// 4 more, since `MULWF`'s product is 8x8 and the shifted high term
    /// only contributes to `hi` mod 2^16. No scratch byte: `MULWF` takes
    /// its operand as a file register, so scale rides W both times and
    /// PROD staging stays inside one instruction pair.
    fn emit_mulwf_scale(&mut self, lo: u16, hi: u16, scale: u16, idx_addr: u16, wide: bool) {
        // `MULWF` multiplies 8x8: larger scales never reach here, the
        // chooser (`mulwf_scale_wins`) routes them to the shift-add chain.
        let scale = u8::try_from(scale)
            .unwrap_or_else(|_| panic!("isel-pic18: MULWF scale {scale} exceeds 255"));
        self.emit(format!("    MOVLW 0x{scale:02X}"));
        let (ia, iff) = self.operand(idx_addr);
        self.emit(format!(
            "    MULWF 0x{iff:03X},{}",
            if ia == 0 { "A" } else { "B" }
        ));
        self.emit("    MOVF 0xFF3,W,A".to_string()); // PRODL
        let (la, lf) = self.operand(lo);
        self.emit(format!(
            "    ADDWF 0x{lf:03X},F,{}",
            if la == 0 { "A" } else { "B" }
        ));
        self.emit("    MOVF 0xFF4,W,A".to_string()); // PRODH
        let (ha, hf) = self.operand(hi);
        self.emit(format!(
            "    ADDWFC 0x{hf:03X},F,{}",
            if ha == 0 { "A" } else { "B" }
        ));
        if wide {
            self.emit(format!("    MOVLW 0x{scale:02X}"));
            let (ia, iff) = self.operand(idx_addr + 1);
            self.emit(format!(
                "    MULWF 0x{iff:03X},{}",
                if ia == 0 { "A" } else { "B" }
            ));
            self.emit("    MOVF 0xFF3,W,A".to_string()); // PRODL << 8
            let (ha, hf) = self.operand(hi);
            self.emit(format!(
                "    ADDWF 0x{hf:03X},F,{}",
                if ha == 0 { "A" } else { "B" }
            ));
        }
    }

    /// Word cost of `emit_mulwf_scale`.
    fn mulwf_scale_words(wide: bool) -> u16 {
        if wide {
            10
        } else {
            6
        }
    }

    /// Whether a scaled term should take the `MULWF` form: at least 2 (a
    /// scale-1 or -0 term cannot need a multiply) and no worse than the
    /// naive 4-words-per-step loop. The chain still wins over it at the
    /// strides where its own cost is lower, so callers rank all three.
    fn mulwf_scale_wins(scale: u16, wide: bool) -> bool {
        scale >= 2
            && u32::from(Self::mulwf_scale_words(wide)) <= 4 * u32::from(scale)
            && scale <= u16::from(u8::MAX)
    }

    /// The only chain candidate: the largest-scale term, and only when
    /// its scale is at least 2 (a scale below 2 cannot win, since the
    /// chain costs at least the initial add plus the seed overhead, and
    /// 0 would underflow the bit math below).
    fn biggest_chainable_term(terms: &[(u16, String)]) -> Option<(usize, u16)> {
        let mut best: Option<(usize, u16)> = None;
        for (i, (scale, _)) in terms.iter().enumerate() {
            if *scale < 2 || best.is_some_and(|(_, bs)| bs >= *scale) {
                continue;
            }
            best = Some((i, *scale));
        }
        best
    }

    /// Pick the term to run through the hardware multiplier or as a
    /// shift-add chain: the largest-scale term that a scaled form beats
    /// the naive loop on by at least 2 words (the chain also carries its
    /// caller's `extra` seed/static overhead; `MULWF` needs none).
    /// `MULWF` is preferred when it is the cheaper of the two at this
    /// scale, and it is width-aware (a 16-bit index costs 4 more words).
    /// Returns the term's index and which form won.
    fn chain_term_index(&self, terms: &[(u16, String)], extra: u16) -> Option<(usize, Scaled)> {
        let (i, scale) = Self::biggest_chainable_term(terms)?;
        let wide = self.reg_width(&terms[i].1) == 2;
        let mut best: Option<(Scaled, u16)> = None;
        if Self::mulwf_scale_wins(scale, wide) {
            best = Some((Scaled::Mulwf, Self::mulwf_scale_words(wide)));
        }
        let chain_extra = if best.is_some() { 0 } else { extra };
        if u32::from(Self::scale_chain_words(scale)) + u32::from(chain_extra) + 2
            <= 4 * u32::from(scale)
        {
            let cost = Self::scale_chain_words(scale) + extra;
            if best.is_none_or(|(_, c)| cost < c) {
                best = Some((Scaled::Chain, cost));
            }
        }
        best.map(|(form, _)| (i, form))
    }

    /// The naive accumulation for every term except `skip`, shared by
    /// the chain-capable setup paths. A 16-bit index register folds its
    /// high byte into every repetition.
    fn add_terms_except(&mut self, terms: &[(u16, String)], skip: Option<usize>, lo: u16, hi: u16) {
        for (i, (scale, reg)) in terms.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            let a = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            for _ in 0..*scale {
                self.emit_fsr_pair_add(lo, hi, a, wide);
            }
        }
    }

    /// If `ptr` resolves to a `const` (flash) global, return
    /// `(table_name, k, terms)` describing where into the table it points;
    /// `None` for a RAM global or slot. A `Val::Global` const is `(name, 0,
    /// [])`; a `Val::Reg` const uses its folded `(Base::Global, k, terms)`
    /// from `resolve_pointers`. This is the flash-side counterpart of
    /// `emit_ptr_setup`: every const read/store routes through it, and a
    /// const `store` panics (ROM is not writable).
    fn const_base_of(&self, ptr: &Val) -> Option<(String, u16, Vec<(u16, String)>)> {
        match ptr {
            Val::Global(g) if self.global_is_const(g) => Some((g.clone(), 0, Vec::new())),
            Val::Reg(r) => match self.resolved_for(r) {
                (Base::Global(name), k, terms) if self.global_is_const(&name) => {
                    Some((name, k, terms))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Seed `TBLPTR = table_base + k + byte_off`: the STATIC part of a
    /// const read. `MOVLW LOW/HIGH/UPPER(table)` load the byte-address
    /// symbol (the table label's low/mid/upper byte, `TBLPTRL/U = 0xF6/F7/
    /// F8`, all SFR segment so `a=0` and no `MOVLB`), then the constant
    /// offset is folded in with carry chains. `TBLPTR` is a BYTE address:
    /// PIC18 program memory is byte-packed (two bytes per 16-bit word),
    /// which is exactly the address `LOW`/`HIGH`/`UPPER` resolve from the
    /// table label's byte address.
    fn emit_tblptr_static(&mut self, table: &str, k: u16, byte_off: u8) {
        for (lit, reg) in [
            (format!("LOW({table})"), 0xF6),
            (format!("HIGH({table})"), 0xF7),
            (format!("UPPER({table})"), 0xF8),
        ] {
            self.emit(format!("    MOVLW {lit}"));
            self.emit(format!("    MOVWF 0x{reg:02X},A"));
        }
        let static_part = k.wrapping_add(u16::from(byte_off));
        if static_part != 0 {
            self.emit(format!("    MOVLW 0x{:02X}", static_part & 0xFF));
            self.emit("    ADDWF 0xF6,F,A".to_string()); // TBLPTRL
            self.emit(format!("    MOVLW 0x{:02X}", static_part >> 8));
            self.emit("    ADDWFC 0xF7,F,A".to_string()); // TBLPTRH
            self.emit("    MOVLW 0x00".to_string());
            self.emit("    ADDWFC 0xF8,F,A".to_string()); // TBLPTRU
        }
    }

    /// Add the single dynamic term (if any) onto `TBLPTR` with carry,
    /// `scale` times. One repetition is `MOVF %reg_lo,W; ADDWF TBLPTRL,F`
    /// seeding the carry, then `TBLPTRH` consumes either the index's real
    /// high byte (`MOVF %reg_hi,W; ADDWFC TBLPTRH,F`) or a zero
    /// (`MOVLW 0`, same word cost), and `TBLPTRU` consumes the running
    /// carry (`ADDWFC TBLPTRU,F`; `W` still holds 0 in the width-1 case).
    /// A 16-bit index needs its high byte folded in or `table[0x1XX]`
    /// reads the wrong byte, which is why `reg_width` is consulted.
    fn add_dynamic_to_tblptr(&mut self, terms: &[(u16, String)]) {
        if let Some((scale, reg)) = terms.first() {
            let lo = self.slot_addr(self.cur_func, reg).direct();
            let wide = self.reg_width(reg) == 2;
            for _ in 0..*scale {
                let (ra, rf) = self.operand(lo);
                self.emit(format!(
                    "    MOVF 0x{rf:03X},W,{}",
                    if ra == 0 { "A" } else { "B" }
                ));
                self.emit("    ADDWF 0xF6,F,A".to_string()); // TBLPTRL += idx_lo
                if wide {
                    let (ha, hf) = self.operand(lo + 1);
                    self.emit(format!(
                        "    MOVF 0x{hf:03X},W,{}",
                        if ha == 0 { "A" } else { "B" }
                    ));
                    self.emit("    ADDWFC 0xF7,F,A".to_string()); // += idx_hi + C
                    self.emit("    MOVLW 0x00".to_string());
                } else {
                    self.emit("    MOVLW 0x00".to_string());
                    self.emit("    ADDWFC 0xF7,F,A".to_string()); // += 0 + C
                }
                self.emit("    ADDWFC 0xF8,F,A".to_string()); // TBLPTRU += C
            }
        }
    }

    /// Word cost of one naive `add_dynamic_to_tblptr` repetition.
    fn tblptr_naive_words(wide: bool) -> u16 {
        if wide {
            6
        } else {
            5
        }
    }

    /// Word cost of the zero-seeded TBLPTR shift-add chain for `scale`:
    /// the 3-byte seed, one index add, per lower bit of `scale` a 4-word
    /// 3-byte doubling, per further set bit an index add, then the table
    /// base bytes and the static part re-joining as literal adds.
    fn tblptr_chain_words(scale: u16, wide: bool, static_part: u16) -> u16 {
        let bits = 16 - scale.leading_zeros() as u16;
        let add = if wide { 6 } else { 5 };
        3 + add
            + (bits - 1) * 4
            + (scale.count_ones() as u16 - 1) * add
            + 6
            + if static_part != 0 { 6 } else { 0 }
    }

    /// Pick the single term for a TBLPTR shift-add chain, the same
    /// largest-scale-2-word-win policy as `chain_term_index`, against
    /// the 3-byte accumulator's costs. Returns the term's index and
    /// scale.
    fn tblptr_chain_term(&self, terms: &[(u16, String)], static_part: u16) -> Option<(usize, u16)> {
        let (i, scale) = Self::biggest_chainable_term(terms)?;
        let wide = self.reg_width(&terms[i].1) == 2;
        (u32::from(Self::tblptr_chain_words(scale, wide, static_part)) + 2
            <= u32::from(Self::tblptr_naive_words(wide)) * u32::from(scale))
        .then_some((i, scale))
    }

    /// Seed `TBLPTR = table_base + k + Σ terms + byte_off` for one flash
    /// byte access: static seeding plus the naive term loop for small
    /// scales, or the zero-seeded shift-add chain when a big stride wins.
    fn emit_tblptr_setup(&mut self, table: &str, k: u16, terms: &[(u16, String)], byte_off: u8) {
        let static_part = k.wrapping_add(u16::from(byte_off));
        if let Some((i, scale)) = self.tblptr_chain_term(terms, static_part) {
            self.emit_tblptr_dynamic_chain(table, static_part, scale, &terms[i].1);
            return;
        }
        self.emit_tblptr_static(table, k, byte_off);
        self.add_dynamic_to_tblptr(terms);
    }

    /// `TBLPTR = table_base + static_part + scale*idx` for a big-enough
    /// stride: zero-seed the triple, run the shift-add chain directly on
    /// it (doublings carry L->H->U; each index add carries the lo byte's
    /// result into `TBLPTRH`, the high byte's or zero's into `TBLPTRU`),
    /// then re-add the table base bytes and the static part as literal
    /// adds. The zero seed is what lets the triple hold the running
    /// product, mirroring the FSR pair's chain shape.
    fn emit_tblptr_dynamic_chain(&mut self, table: &str, static_part: u16, scale: u16, reg: &str) {
        let lo = self.slot_addr(self.cur_func, reg).direct();
        let wide = self.reg_width(reg) == 2;
        self.emit("    CLRF 0xF6,A".to_string()); // TBLPTRL = 0
        self.emit("    CLRF 0xF7,A".to_string()); // TBLPTRH = 0
        self.emit("    CLRF 0xF8,A".to_string()); // TBLPTRU = 0
        let emit_idx_add = |g: &mut Self| {
            let (ra, rf) = g.operand(lo);
            g.emit(format!(
                "    MOVF 0x{rf:03X},W,{}",
                if ra == 0 { "A" } else { "B" }
            ));
            g.emit("    ADDWF 0xF6,F,A".to_string());
            if wide {
                let (ha, hf) = g.operand(lo + 1);
                g.emit(format!(
                    "    MOVF 0x{hf:03X},W,{}",
                    if ha == 0 { "A" } else { "B" }
                ));
                g.emit("    ADDWFC 0xF7,F,A".to_string());
                g.emit("    MOVLW 0x00".to_string());
            } else {
                g.emit("    MOVLW 0x00".to_string());
                g.emit("    ADDWFC 0xF7,F,A".to_string());
            }
            g.emit("    ADDWFC 0xF8,F,A".to_string());
        };
        emit_idx_add(self);
        let bits = 16 - scale.leading_zeros();
        for i in (0..bits - 1).rev() {
            self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS,C = 0
            self.emit("    RLCF 0xF6,F,A".to_string());
            self.emit("    RLCF 0xF7,F,A".to_string());
            self.emit("    RLCF 0xF8,F,A".to_string());
            if (scale >> i) & 1 == 1 {
                emit_idx_add(self);
            }
        }
        // The table base re-joins after the chain as literal adds
        // (MOVLW never touches C, so the carry chain stays intact).
        for (lit, reg) in [
            (format!("LOW({table})"), "0xF6"),
            (format!("HIGH({table})"), "0xF7"),
            (format!("UPPER({table})"), "0xF8"),
        ] {
            self.emit(format!("    MOVLW {lit}"));
            let op = if reg == "0xF6" { "ADDWF" } else { "ADDWFC" };
            self.emit(format!("    {op} {reg},F,A"));
        }
        if static_part != 0 {
            self.emit(format!("    MOVLW 0x{:02X}", static_part & 0xFF));
            self.emit("    ADDWF 0xF6,F,A".to_string());
            self.emit(format!("    MOVLW 0x{:02X}", static_part >> 8));
            self.emit("    ADDWFC 0xF7,F,A".to_string());
            self.emit("    MOVLW 0x00".to_string());
            self.emit("    ADDWFC 0xF8,F,A".to_string());
        }
    }

    /// One `const` (flash) byte read: `TBLPTR = table_base + k + Σ terms +
    /// byte_off`, `TBLRD*` (no auto-increment: per-byte re-setup keeps
    /// every read independent, mirroring the pointer lowering's per-byte FSR0 re-setup), then
    /// `MOVFF TABLAT, dst`. Multi-byte loads call this once per byte with
    /// an increasing `byte_off`.
    fn emit_const_load_byte(
        &mut self,
        table: &str,
        k: u16,
        terms: &[(u16, String)],
        byte_off: u8,
        dst: u16,
    ) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P4 scope; {} terms)",
            terms.len()
        );
        self.emit_tblptr_setup(table, k, terms, byte_off);
        self.emit("    TBLRD*".to_string());
        self.emit_copy_byte(0xFF5, dst); // TABLAT -> dst
    }

    /// Copy each call arg into the callee's `{func}::{param}` slots. Shared
    /// by the direct call path and the per-candidate arms of an indirect
    /// call chain. (epic-cc#73)
    fn emit_call_args(&mut self, func: &str, args: &[ir::CallArg]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel-pic18: call to unknown function @{func}"));
        let named = callee.params.len();
        let mut va_off: u16 = 0;
        for (i, arg) in args.iter().enumerate() {
            if i >= named {
                // Extra (variadic) arg: lands in the callee's `__va`
                // region at the running offset. (epic-cc#131)
                let va = self
                    .addrs
                    .get(&ssa_key(&func, "__va"))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!("isel-pic18: variadic call to non-variadic @{func} (no __va region)")
                    });
                let aty = arg
                    .ty
                    .expect("isel-pic18: scalar variadic arg must carry a type");
                let aw = u16::from(aty.bytes());
                self.emit_move_val_to_slot(&arg.val, aty, va + va_off);
                va_off += aw;
                continue;
            }
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(&func, pname).direct();
            if let Some(size) = arg.byval {
                let src_ptr = match &arg.val {
                    Val::Const(_) => {
                        panic!("isel-pic18: const byval call arg not yet supported")
                    }
                    other => other.clone(),
                };
                for b in 0..size {
                    match self.emit_ptr_setup(&src_ptr, b) {
                        Addr::Direct(src) => self.emit_copy_byte(src, pa + u16::from(b)),
                        Addr::Indirect => {
                            self.emit(format!("    MOVFF 0xFEF, 0x{:03X}", pa + u16::from(b)));
                        }
                    }
                }
            } else if arg.sret {
                // `sret` stores the 2-byte address `arg.val` points to in the slot.
                // A literal has no meaning as an address source. `Addr::Direct`
                // writes the two bytes as literals; `Addr::Indirect` means FSR0
                // holds the resolved runtime address, so the slot takes FSR0L/FSR0H
                // via `MOVFF`. Resolves through `emit_ptr_setup` like other consumers.
                assert!(
                    !matches!(arg.val, Val::Const(_)),
                    "isel-pic18: const sret call arg not yet supported"
                );
                if let Some(sa) = self.direct_ptr_forward_src(&arg.val) {
                    self.emit_copy_byte(sa, pa);
                    self.emit_copy_byte(sa + 1, pa + 1);
                } else {
                    match self.emit_ptr_setup(&arg.val, 0) {
                        Addr::Direct(addr) => {
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                            let (a0, f0) = self.operand(pa);
                            self.emit(format!(
                                "    MOVWF 0x{f0:03X},{}",
                                if a0 == 0 { "A" } else { "B" }
                            ));
                            self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                            let (a1, f1) = self.operand(pa + 1);
                            self.emit(format!(
                                "    MOVWF 0x{f1:03X},{}",
                                if a1 == 0 { "A" } else { "B" }
                            ));
                        }
                        Addr::Indirect => {
                            self.emit_copy_byte(0xFE9, pa); // FSR0L -> sret slot lo
                            self.emit_copy_byte(0xFEA, pa + 1); // FSR0H -> sret slot hi
                        }
                    }
                }
            } else if arg.ty.is_none() {
                // A plain `ptr` arg carries no scalar type: pass the
                // resolved 2-byte address, the same shape `sret` uses.
                assert!(
                    !arg.sret && arg.byval.is_none(),
                    "isel-pic18: plain ptr arg must be non-sret/non-byval"
                );
                assert_eq!(
                    callee.params[i].width, 2,
                    "isel-pic18: callee ptr param must be 2 bytes"
                );
                if let Val::Const(k) = arg.val {
                    assert_eq!(k, 0, "isel-pic18: non-zero const ptr not supported");
                    let (a0, f0) = self.operand(pa);
                    self.emit(format!(
                        "    CLRF 0x{f0:03X},{}",
                        if a0 == 0 { "A" } else { "B" }
                    ));
                    let (a1, f1) = self.operand(pa + 1);
                    self.emit(format!(
                        "    CLRF 0x{f1:03X},{}",
                        if a1 == 0 { "A" } else { "B" }
                    ));
                } else if let Val::Global(g) = &arg.val {
                    if self.is_function(g) {
                        // A function's address is a link-time label literal:
                        // byte 0 = LOW(g), byte 1 = HIGH(g). A
                        // param-forwarded callback arrives as
                        // such an arg. (epic-cc#73) (epic-cc#137)
                        let (a0, f0) = self.operand(pa);
                        self.emit(format!("    MOVLW LOW({g})"));
                        self.emit(format!(
                            "    MOVWF 0x{f0:03X},{}",
                            if a0 == 0 { "A" } else { "B" }
                        ));
                        let (a1, f1) = self.operand(pa + 1);
                        self.emit(format!("    MOVLW HIGH({g})"));
                        self.emit(format!(
                            "    MOVWF 0x{f1:03X},{}",
                            if a1 == 0 { "A" } else { "B" }
                        ));
                    } else {
                        let addr = self.global_addr(g);
                        let (a0, f0) = self.operand(pa);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit(format!(
                            "    MOVWF 0x{f0:03X},{}",
                            if a0 == 0 { "A" } else { "B" }
                        ));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        let (a1, f1) = self.operand(pa + 1);
                        self.emit(format!(
                            "    MOVWF 0x{f1:03X},{}",
                            if a1 == 0 { "A" } else { "B" }
                        ));
                    }
                } else if let Val::Reg(r) = &arg.val {
                    // A runtime pointer value (a `load ptr` result, e.g.
                    // the taskmgr `t->arg` field): the two address bytes
                    // live in the reg's slot. Copy them into the param
                    // slot; the callee's FSR-based deref resolves the
                    // address at runtime. (epic-cc#155)
                    if !self.resolved.contains_key(&ssa_key(self.cur_func, r)) {
                        let sa = self.slot_addr(self.cur_func, r).direct();
                        self.emit_copy_byte(sa, pa);
                        self.emit_copy_byte(sa + 1, pa + 1);
                    } else if let Some(sa) = self.direct_ptr_forward_src(&arg.val) {
                        self.emit_copy_byte(sa, pa);
                        self.emit_copy_byte(sa + 1, pa + 1);
                    } else {
                        match self.emit_ptr_setup(&arg.val, 0) {
                            Addr::Direct(addr) => {
                                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                                let (a0, f0) = self.operand(pa);
                                self.emit(format!(
                                    "    MOVWF 0x{f0:03X},{}",
                                    if a0 == 0 { "A" } else { "B" }
                                ));
                                self.emit(format!(
                                    "    MOVLW 0x{:02X}",
                                    ((addr >> 8) & 0xFF) as u8
                                ));
                                let (a1, f1) = self.operand(pa + 1);
                                self.emit(format!(
                                    "    MOVWF 0x{f1:03X},{}",
                                    if a1 == 0 { "A" } else { "B" }
                                ));
                            }
                            Addr::Indirect => {
                                self.emit_copy_byte(0xFE9, pa); // FSR0L -> param slot lo
                                self.emit_copy_byte(0xFEA, pa + 1); // FSR0H -> param slot hi
                            }
                        }
                    }
                } else {
                    match self.emit_ptr_setup(&arg.val, 0) {
                        Addr::Direct(addr) => {
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                            let (a0, f0) = self.operand(pa);
                            self.emit(format!(
                                "    MOVWF 0x{f0:03X},{}",
                                if a0 == 0 { "A" } else { "B" }
                            ));
                            self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                            let (a1, f1) = self.operand(pa + 1);
                            self.emit(format!(
                                "    MOVWF 0x{f1:03X},{}",
                                if a1 == 0 { "A" } else { "B" }
                            ));
                        }
                        Addr::Indirect => {
                            self.emit_copy_byte(0xFE9, pa); // FSR0L -> param slot lo
                            self.emit_copy_byte(0xFEA, pa + 1); // FSR0H -> param slot hi
                        }
                    }
                }
            } else {
                let ty = arg
                    .ty
                    .expect("isel-pic18: scalar call arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, ty, pa);
                // the float differential conversion ABI: __uitofp_f32/__sitofp_f32 take a 4-byte val slot
                // but i8/i16 sources copy only their width; stale high bytes corrupt
                // the leading-1 search (the conversion fill: isel/src/lib.rs:1618-1661). Fill them.
                if (ty.bytes() as u16) < u16::from(callee.params[i].width) {
                    assert_eq!(
                        callee.params[i].width, 4,
                        "isel-pic18: narrow scalar arg {} of @{} into non-4-byte param",
                        i, func
                    );
                    let aw = ty.bytes() as u16;
                    // The fill addresses the callee's param slot through
                    // operand(): a routine frame lives in one GPR bank
                    // (round_if_routine), so the bank select rides the
                    // first operand and no MOVLB can land between the
                    // BTFSC and its skipped MOVLW.
                    match func {
                        "__uitofp_f32" => {
                            for j in aw..4 {
                                self.emit_banked("CLRF", pa + j, "");
                            }
                        }
                        "__sitofp_f32" => {
                            let sign = pa + aw - 1;
                            if aw == 2 {
                                self.emit_banked("MOVF", sign, ",W");
                                self.emit_banked("MOVWF", pa + 2, "");
                                self.emit_banked("MOVWF", pa + 3, "");
                            } else {
                                assert_eq!(
                                    aw, 1,
                                    "isel-pic18: unexpected narrow width for @__sitofp_f32"
                                );
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit_banked("BTFSC", sign, ", 7");
                                self.emit("    MOVLW 0xFF".to_string());
                                for j in 1..4 {
                                    self.emit_banked("MOVWF", pa + j, "");
                                }
                            }
                        }
                        other => {
                            panic!("isel-pic18: narrow scalar arg into wide param of @{other}")
                        }
                    }
                }
            }
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        self.cur_loc = i.loc().cloned();
        match i {
            Inst::Load(l) => {
                // An i1 global is real: clang's own -O1 GlobalOpt narrows an
                // internal flag only ever written 0/1 down to `global i1`
                // (epic-cc#462). i1 is one byte in the byte model, so the
                // byte loop below is the whole story. The literal-pointer
                // arm copies an SFR byte raw (MOVFF cannot mask), which
                // stays sound only while that byte is 0/1: the same
                // exactly-0/1 premise the Zext and Sext arms state.
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                // Literal-pointer (SFR) load: `inttoptr` form, a direct
                // physical address: MOVFF copies byte-wise with no access
                // bit and no BSR involvement.
                if l.ptr.starts_with("0x") {
                    let base = self.literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit_copy_byte(base + u16::from(k), dst + u16::from(k));
                    }
                    return;
                }
                let ptr_val = if let Some(g) = l.ptr.strip_prefix('@') {
                    Val::Global(g.to_string())
                } else if let Some(r) = l.ptr.strip_prefix('%') {
                    Val::Reg(r.to_string())
                } else {
                    panic!("isel-pic18: malformed load pointer operand {:?}", l.ptr);
                };
                // the flash const: a load through a `const` (flash) base reads via
                // TBLRD, one independent read per byte. Everything else
                // (RAM globals, allocas, sret slots, dynamic pointers)
                // keeps the integer-spine/pointer-lowering FSR/INDF path.
                if let Some((table, k, terms)) = self.const_base_of(&ptr_val) {
                    for kk in 0..l.ty.bytes() {
                        self.emit_const_load_byte(&table, k, &terms, kk, dst + u16::from(kk));
                    }
                    return;
                }
                // FSR0 seeds once (byte_off 0); indirect bytes walk
                // POSTINC0 (this loop is the whole ordering contract
                // ADR-009 needed, epic-cc#471). Direct addresses are
                // linear in byte_off, so base_addr + k already matches
                // re-resolving at byte_off = k.
                let n = l.ty.bytes();
                // Byte-indexed small-array reads go through `PLUSW0` (one
                // `LFSR` per resident run, then `MOVF idx,W` +
                // `MOVFF PLUSW0,dst` per byte) instead of the 16-bit FSR
                // add around `INDF0`. Raw emission, never the staged copy
                // path: a staged `PLUSW0` read would replay against a later
                // `FSR0`/`W`. (epic-cc#665)
                if let Some(shape) = self.plusw_shape(&ptr_val) {
                    for k in 0..n {
                        self.emit_plusw_setup(&shape, k);
                        let d = dst + u16::from(k);
                        self.invalidate_fsr0_if_slot_written(d, 1);
                        self.emit(format!("    MOVFF 0xFEB, 0x{d:03X}"));
                    }
                } else {
                    match self.emit_ptr_setup(&ptr_val, 0) {
                        Addr::Direct(base_addr) => {
                            for k in 0..n {
                                self.emit_copy_byte(base_addr + u16::from(k), dst + u16::from(k));
                            }
                        }
                        Addr::Indirect => {
                            for k in 0..n {
                                let reg = if k + 1 == n { 0xFEF } else { 0xFEE }; // INDF0 : POSTINC0
                                self.emit(format!(
                                    "    MOVFF 0x{reg:03X}, 0x{:03X}",
                                    dst + u16::from(k)
                                ));
                            }
                            self.bump_fsr0_tracked_offset(n);
                        }
                    }
                }
            }
            Inst::Store(s) => {
                // Same i1-in-memory story as the Load arm above (epic-cc#462);
                // `trunc` normalizes an i1 byte to 0/1, so the stored byte
                // keeps the convention every i1 consumer relies on.
                // Literal-pointer (SFR) store: `inttoptr` form, a direct
                // physical address. A register/global source copies via
                // MOVFF (no access bit); a constant goes through W with
                // `operand`'s access-bit (a=0 for the SFR segment, no
                // MOVLB).
                if s.ptr.starts_with("0x") {
                    let base = self.literal_ptr_addr(&s.ptr);
                    match &s.val {
                        Val::Const(_) => self.emit_move_val_to_slot(&s.val, s.ty, base),
                        _ => {
                            let src = self.val_addr(&s.val).direct();
                            for i in 0..s.ty.bytes() {
                                self.emit_copy_byte(src + u16::from(i), base + u16::from(i));
                            }
                        }
                    }
                    return;
                }
                let ptr_val = if let Some(g) = s.ptr.strip_prefix('@') {
                    Val::Global(g.to_string())
                } else if let Some(r) = s.ptr.strip_prefix('%') {
                    Val::Reg(r.to_string())
                } else {
                    panic!("isel-pic18: malformed store pointer operand {:?}", s.ptr);
                };
                // the flash const: a store through a `const` (flash) base is a write to
                // ROM. It must panic (matching PIC14's
                // store-through-const panic), never silently emit a
                // MOVFF/MOVWF that the simulator would apply to a RAM
                // alias of the same low address.
                if self.const_base_of(&ptr_val).is_some() {
                    panic!(
                        "isel-pic18: ROM is not writable: store through const global {ptr_val:?}"
                    );
                }
                // Direct values cover the whole slot through one setup.
                // Indirect bytes seed FSR0 once and walk POSTINC0 (same
                // single-loop ordering contract as the Load arm above,
                // epic-cc#471). Each source byte still materializes into
                // W (literals directly, registers via MOVF) before the
                // write.
                // Byte-indexed small-array stores go through `PLUSW0` like
                // the Load arm above: `MOVF idx,W` then
                // `MOVFF value,PLUSW0`, which preserves `W` (the write
                // collision ADR-009 item 4 feared). Only register values
                // with a slot move directly; address-valued regs and
                // constants keep the `INDF0` lowering. (epic-cc#665)
                let val_slot = match &s.val {
                    Val::Reg(r) => self.addrs.get(&ssa_key(self.cur_func, r)).copied(),
                    _ => None,
                };
                if let (Some(shape), Some(vslot)) = (self.plusw_shape(&ptr_val), val_slot) {
                    let n = s.ty.bytes();
                    for k in 0..n {
                        self.emit_plusw_setup(&shape, k);
                        self.emit(format!("    MOVFF 0x{:03X}, 0xFEB", vslot + u16::from(k)));
                    }
                } else {
                    match self.emit_ptr_setup(&ptr_val, 0) {
                        Addr::Direct(dst) => self.emit_move_val_to_slot(&s.val, s.ty, dst),
                        Addr::Indirect => {
                            let n = s.ty.bytes();
                            for k in 0..n {
                                self.emit_load_w(&s.val, k, false);
                                let reg = if k + 1 == n { 0xFEF } else { 0xFEE }; // INDF0 : POSTINC0
                                self.emit(format!("    MOVWF 0x{reg:03X},A"));
                            }
                            self.bump_fsr0_tracked_offset(n);
                        }
                    }
                }
            }
            Inst::Bin(b) => {
                let n = b.ty.bytes();

                assert!(
                    n == 1 || n == 2 || n == 4,
                    "isel-pic18: only i8/i16/i32 Bin ops implemented (n={n})"
                );
                // the constant-count shifts: a const count inlines as a fixed
                // RLCF/RRCF sequence; k == 0 is a plain copy; k >= width is
                // LLVM poison and panics. A variable (reg) count must
                // never reach isel: legalize rewrites it to a routine call.
                // Without this arm a shift would hit the `(other, _)`
                // panic below.
                let av = self.val_addr(&b.a).direct();
                let dst = self.slot_addr(self.cur_func, &b.dst).direct();
                if matches!(b.op, ir::BinOp::Shl | ir::BinOp::LShr | ir::BinOp::AShr) {
                    let width = i64::from(n) * 8;
                    let k = match &b.b {
                        Val::Const(k) => *k,
                        other => panic!(
                            "isel-pic18: variable-count {:?} shift reached isel (count {other:?}); legalize must rewrite it to a routine call",
                            b.op
                        ),
                    };
                    assert!(
                        (0..width).contains(&k),
                        "isel-pic18: const shift count {k} out of range [0, {width}) (LLVM poison)"
                    );
                    // No barrel shifter: RLCF/RRCF rotate one bit, so a
                    // multiple-of-8 shift is just a byte move (MOVFF), not
                    // 8 rotates per byte. Split k = 8*m + r: move the m
                    // surviving bytes, fill the m vacated ones (zero, or
                    // the sign byte for ashr), then bit-rotate only the
                    // residual r over the live bytes -- the filled ones
                    // are quiescent under further rotation. epic-cc#470.
                    let m = (k / 8) as u16;
                    let r = k % 8;
                    let n16 = u16::from(n);
                    if m == 0 {
                        // Sub-byte shift only: no whole-byte move to
                        // make, so copy the operand and rotate every
                        // byte.
                        self.emit_move_val_to_slot(&b.a, b.ty, dst);
                    } else {
                        // The byte-move path reads the operand straight
                        // out of RAM, but `val_addr` maps a literal to
                        // the truncated address k & 0xFF: a const-LHS
                        // shift would move whatever bytes live at that
                        // address. Fail loudly like the other const-LHS
                        // arms.
                        assert!(
                            !matches!(b.a, Val::Const(_)),
                            "isel-pic18: const-LHS byte-granular shift (constant as the first operand) not yet supported"
                        );
                        match b.op {
                            ir::BinOp::Shl => {
                                // dst[n-1..m] = a[n-1-m..0]; dst[m-1..0] = 0.
                                // High-to-low so an in-place shift (av ==
                                // dst) never reads a byte already overwritten.
                                for i in (m..n16).rev() {
                                    self.emit_copy_byte(av + (i - m), dst + i);
                                }
                                for i in 0..m {
                                    self.emit_banked("CLRF", dst + i, "");
                                }
                            }
                            ir::BinOp::LShr | ir::BinOp::AShr => {
                                // dst[0..n-m) = a[m..n); dst[n-m..n) = 0
                                // (lshr) or the sign fill (ashr). Low-to-high
                                // is the mirror in-place safety argument.
                                for i in 0..(n16 - m) {
                                    self.emit_copy_byte(av + i + m, dst + i);
                                }
                                if matches!(b.op, ir::BinOp::AShr) {
                                    let (ha, hf) = self.operand(av + n16 - 1);
                                    let hbank = if ha == 0 { "A" } else { "B" };
                                    self.emit("    MOVLW 0x00".to_string());
                                    self.emit(format!("    BTFSC 0x{hf:03X},7,{hbank}"));
                                    self.emit("    MOVLW 0xFF".to_string());
                                    for i in (n16 - m)..n16 {
                                        self.emit_banked("MOVWF", dst + i, "");
                                    }
                                } else {
                                    for i in (n16 - m)..n16 {
                                        self.emit_banked("CLRF", dst + i, "");
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                    let active = n16 - m;
                    // Nibble-boundary shifts have shorter, sim-verified
                    // forms than the per-bit unroll (crates/superopt, docs/41):
                    // one lane shifts by 4-7 as SWAPF+mask plus one seeded
                    // rotate per extra bit (epic-cc#573), an in-place 16-bit
                    // pair has a canned construction per amount 4-7 (left) or
                    // at amount 4 (right), each checked over its full
                    // input domain. They clobber W, dead at statement
                    // entry (no lowering reads W before writing it; W tracking
                    // is #502), and STATUS no worse than the unroll they
                    // replace.
                    if b.op == ir::BinOp::LShr && (4..=7).contains(&r) {
                        if active == 1 {
                            // Mirror of the single-lane left form: SWAPF
                            // then keep the low nibble. The lane is `dst`
                            // (right shifts keep dst[0..n-m)), not `dst + m`
                            // as the left form uses; the byte-move above
                            // already put the surviving byte there, and no
                            // higher lane rotates bits in, so the residual
                            // is a plain nibble down-shift. Amounts 5-7
                            // (epic-cc#573) add one carry-seeded rotate per
                            // extra bit; each needs its own BCF, same as
                            // the left form, since a lone lane takes carry
                            // from nowhere.
                            self.emit_banked("SWAPF", dst, ",W");
                            self.emit("    ANDLW 0x0F".to_string());
                            self.emit_banked("MOVWF", dst, "");
                            for _ in 4..r {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                self.emit_banked("RRCF", dst, ",F");
                            }
                            return;
                        }
                        if r == 4 && n16 == 2 && m == 0 {
                            // lo' = (lo>>4) | ((hi&0x0F)<<4), hi' = hi>>4, both
                            // as nibble-swaps plus a mask. W-only, so the same
                            // dead-W precondition as the left forms.
                            self.emit_banked("SWAPF", dst, ",F");
                            self.emit("    MOVLW 0x0F".to_string());
                            self.emit_banked("ANDWF", dst, ",F");
                            self.emit_banked("SWAPF", dst + 1, ",W");
                            self.emit("    ANDLW 0xF0".to_string());
                            self.emit_banked("IORWF", dst, ",F");
                            self.emit_banked("SWAPF", dst + 1, ",W");
                            self.emit("    ANDLW 0x0F".to_string());
                            self.emit_banked("MOVWF", dst + 1, "");
                            return;
                        }
                    }
                    if b.op == ir::BinOp::Shl && r > 0 {
                        if active == 1 && (4..=7).contains(&r) {
                            self.emit_banked("SWAPF", dst + m, ",W");
                            self.emit("    ANDLW 0xF0".to_string());
                            self.emit_banked("MOVWF", dst + m, "");
                            // Amounts 5-7 (epic-cc#573): one BCF-seeded
                            // rotate per extra bit. The seed cannot be
                            // shared across steps the way the 16-bit
                            // amount-5 arm shares one BCF across lanes: a
                            // lone lane's bit 0 must read 0 every step.
                            for _ in 4..r {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                self.emit_banked("RLCF", dst + m, ",F");
                            }
                            return;
                        }
                        if n16 == 2 && m == 0 && (4..=7).contains(&r) {
                            // Every lane op fetches its operand fresh so a
                            // dst straddling a bank boundary re-selects BSR
                            // exactly where the unrolled loop would; caching
                            // both lanes' bank letters up front misaddresses
                            // the far lane.
                            match r {
                                4 | 5 => {
                                    self.emit_banked("SWAPF", dst + 1, ",F");
                                    self.emit("    MOVLW 0xF0".to_string());
                                    self.emit_banked("ANDWF", dst + 1, ",F");
                                    self.emit_banked("SWAPF", dst, ",W");
                                    self.emit("    ANDLW 0x0F".to_string());
                                    self.emit_banked("IORWF", dst + 1, ",F");
                                    self.emit_banked("SWAPF", dst, ",F");
                                    self.emit("    MOVLW 0xF0".to_string());
                                    self.emit_banked("ANDWF", dst, ",F");
                                    if r == 5 {
                                        self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                        self.emit_banked("RLCF", dst, ",F");
                                        self.emit_banked("RLCF", dst + 1, ",F");
                                    }
                                }
                                6 => {
                                    self.emit_banked("RRNCF", dst + 1, ",F");
                                    self.emit_banked("RRNCF", dst + 1, ",F");
                                    self.emit("    MOVLW 0xC0".to_string());
                                    self.emit_banked("ANDWF", dst + 1, ",F");
                                    self.emit_banked("RRNCF", dst, ",F");
                                    self.emit_banked("RRNCF", dst, ",F");
                                    self.emit_banked("MOVF", dst, ",W");
                                    self.emit("    ANDLW 0x3F".to_string());
                                    self.emit_banked("IORWF", dst + 1, ",F");
                                    self.emit("    MOVLW 0xC0".to_string());
                                    self.emit_banked("ANDWF", dst, ",F");
                                }
                                7 => {
                                    self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                    self.emit_banked("RRCF", dst + 1, ",F");
                                    self.emit_banked("RRCF", dst, ",F");
                                    self.emit_banked("MOVF", dst, ",W");
                                    self.emit_banked("MOVWF", dst + 1, "");
                                    self.emit_banked("CLRF", dst, "");
                                    self.emit_banked("RRCF", dst, ",F");
                                }
                                _ => unreachable!(),
                            }
                            return;
                        }
                        // 4-lane fused forms for amounts 6 and 7, verified
                        // in crates/superopt (#549): rotate each byte right
                        // by 8-r, then recombine high-to-low, each lane's
                        // high r bits from its own byte and low 8-r bits from
                        // the byte below. 25 words at r=6, 21 at r=7, against
                        // the 30/35-word unroll. W-only. Only 6 and 7 clear
                        // the `5r` unroll, so r=4/5 stay on their own arms.
                        if n16 == 4 && m == 0 && (r == 6 || r == 7) {
                            let rot = 8 - r;
                            let (himask, lomask) = if r == 6 { (0xC0, 0x3F) } else { (0x80, 0x7F) };
                            for _ in 0..rot {
                                for b in (0..4u16).rev() {
                                    self.emit_banked("RRNCF", dst + b, ",F");
                                }
                            }
                            for b in (1..4u16).rev() {
                                self.emit(format!("    MOVLW 0x{himask:02X}"));
                                self.emit_banked("ANDWF", dst + b, ",F");
                                self.emit_banked("MOVF", dst + b - 1, ",W");
                                self.emit(format!("    ANDLW 0x{lomask:02X}"));
                                self.emit_banked("IORWF", dst + b, ",F");
                            }
                            self.emit(format!("    MOVLW 0x{himask:02X}"));
                            self.emit_banked("ANDWF", dst, ",F");
                            return;
                        }
                    }
                    if b.op == ir::BinOp::LShr && r > 0 {
                        // 4-lane fused forms for amounts 6 and 7 (#551),
                        // mirroring the left family: rotate each byte left
                        // by 8-r, then recombine low-to-high. 25 words at
                        // r=6 and 21 at r=7, against the 30/35-word unroll;
                        // W-only, and the ascending combine reads each
                        // upper neighbour before the walk rewrites it.
                        if n16 == 4 && m == 0 && (r == 6 || r == 7) {
                            let rot = 8 - r;
                            let (himask, lomask) = if r == 6 { (0xFC, 0x03) } else { (0xFE, 0x01) };
                            for _ in 0..rot {
                                for b in (0..4u16).rev() {
                                    self.emit_banked("RLNCF", dst + b, ",F");
                                }
                            }
                            for b in 0..3u16 {
                                self.emit(format!("    MOVLW 0x{lomask:02X}"));
                                self.emit_banked("ANDWF", dst + b, ",F");
                                self.emit(format!("    MOVLW 0x{himask:02X}"));
                                self.emit_banked("ANDWF", dst + b + 1, ",W");
                                self.emit_banked("IORWF", dst + b, ",F");
                            }
                            self.emit(format!("    MOVLW 0x{lomask:02X}"));
                            self.emit_banked("ANDWF", dst + 3, ",F");
                            return;
                        }
                    }
                    for _ in 0..r {
                        match b.op {
                            ir::BinOp::Shl => {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in 0..active {
                                    let (da, df) = self.operand(dst + m + i);
                                    let dbank = if da == 0 { "A" } else { "B" };
                                    self.emit(format!("    RLCF 0x{df:03X},F,{dbank}"));
                                }
                            }
                            ir::BinOp::LShr => {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in (0..active).rev() {
                                    let (da, df) = self.operand(dst + i);
                                    let dbank = if da == 0 { "A" } else { "B" };
                                    self.emit(format!("    RRCF 0x{df:03X},F,{dbank}"));
                                }
                            }
                            ir::BinOp::AShr => {
                                let hi = dst + active - 1;
                                let (ha, hf) = self.operand(hi);
                                let hbank = if ha == 0 { "A" } else { "B" };
                                self.emit(format!("    BTFSC 0x{hf:03X},7,{hbank}"));
                                self.emit("    BSF 0xFD8,0,A".to_string()); // STATUS C
                                self.emit(format!("    BTFSS 0x{hf:03X},7,{hbank}"));
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in (0..active).rev() {
                                    let (da, df) = self.operand(dst + i);
                                    let dbank = if da == 0 { "A" } else { "B" };
                                    self.emit(format!("    RRCF 0x{df:03X},F,{dbank}"));
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                    return;
                }
                // PIC18 port of PIC14's `emit_sub_const_lhs` / `emit_commutative`:
                // a const LHS would be misread as a RAM address via
                // `val_addr` (`k & 0xFF` truncates), so handle it here.
                // Commutative ops swap; `k - a` uses `SUBLW` for byte 0 and
                // the INCFSZ/BTFSS carry fold for bytes 1..n.
                if let Val::Const(k) = b.a {
                    let n = b.ty.bytes();
                    let dst = self.slot_addr(self.cur_func, &b.dst).direct();
                    match b.op {
                        ir::BinOp::Add | ir::BinOp::And | ir::BinOp::Or | ir::BinOp::Xor => {
                            // Commutative: `k op x` == `x op k`, reuse the
                            // normal path with swapped operands.
                            let swapped = ir::Bin {
                                op: b.op,
                                ty: b.ty,
                                dst: b.dst.clone(),
                                a: b.b.clone(),
                                b: Val::Const(k),
                                loc: b.loc.clone(),
                            };
                            // Re-enter the Bin arm with swapped operands via
                            // the normal per-byte loop: emit the swapped bin
                            // directly here to avoid recursion.
                            let av = self.val_addr(&swapped.a).direct();
                            for i in 0..n {
                                self.emit_load_w(&swapped.b, i, false);
                                let carry =
                                    i > 0 && matches!(swapped.op, ir::BinOp::Add | ir::BinOp::Sub);
                                let mne = match (swapped.op, carry) {
                                    (ir::BinOp::Add, false) => "ADDWF",
                                    (ir::BinOp::Add, true) => "ADDWFC",
                                    (ir::BinOp::Sub, false) => "SUBWF",
                                    (ir::BinOp::Sub, true) => "SUBFWB",
                                    (ir::BinOp::And, _) => "ANDWF",
                                    (ir::BinOp::Or, _) => "IORWF",
                                    (ir::BinOp::Xor, _) => "XORWF",
                                    _ => unreachable!(),
                                };
                                let (aacc, af) = self.operand(av + u16::from(i));
                                let abank = if aacc == 0 { "A" } else { "B" };
                                self.emit(format!("    {mne} 0x{af:03X},W,{abank}"));
                                self.emit_w_store(dst + u16::from(i));
                            }
                            return;
                        }
                        ir::BinOp::Sub => {
                            // `k - a` for `n > 1`: byte 0 via `SUBLW`, upper bytes via the
                            // borrow-aware chain. `C0` parks in flag bit (0x0000,0) of the
                            // reserved return region: the bit stays live-free across the `Bin`,
                            // so an interrupt mid-sequence cannot clobber a live local before
                            // `COMF`/`ADDLW` overwrite `STATUS`.
                            let aa = self.val_addr(&b.b).direct();
                            let dst = self.slot_addr(self.cur_func, &b.dst).direct();
                            let (aacc0, af0) = self.operand(aa);
                            let abank0 = if aacc0 == 0 { "A" } else { "B" };
                            self.emit(format!("    MOVF 0x{af0:03X},W,{abank0}"));
                            self.emit(format!("    SUBLW 0x{:02X}", (k & 0xFF) as u8));
                            let (dacc0, df0) = self.operand(dst);
                            let dbank0 = if dacc0 == 0 { "A" } else { "B" };
                            self.emit(format!("    MOVWF 0x{df0:03X},{dbank0}"));
                            for i in 1..n {
                                let kb = ((k >> (u32::from(i) * 8)) & 0xFF) as u8;
                                self.emit("    BTFSC 0xFD8,0,A".to_string());
                                self.emit("    BSF 0x0000,0,A".to_string());
                                self.emit("    BTFSS 0xFD8,0,A".to_string());
                                self.emit("    BCF 0x0000,0,A".to_string());
                                let (aacc, af) = self.operand(aa + u16::from(i));
                                let abank = if aacc == 0 { "A" } else { "B" };
                                self.emit(format!("    COMF 0x{af:03X},W,{abank}"));
                                // A zero lane adds nothing to `~a`, so its `ADDLW`
                                // is dead: with the saved bit clear the untouched
                                // `C` already equals the correct carry out (adding
                                // zero cannot carry), and with it set the
                                // `ADDLW 0x01` below recomputes it (epic-cc#575).
                                if kb != 0 {
                                    self.emit(format!("    ADDLW 0x{kb:02X}"));
                                }
                                self.emit("    BTFSC 0x0000,0,A".to_string());
                                self.emit("    ADDLW 0x01".to_string());
                                let (dacc, df) = self.operand(dst + u16::from(i));
                                let dbank = if dacc == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVWF 0x{df:03X},{dbank}"));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                assert!(
                    !matches!(b.a, Val::Const(_)),
                    "isel-pic18: const-LHS Bin (constant as the first operand) not yet supported  -  needs the isel::emit_sub_const_lhs-equivalent handling"
                );
                for i in 0..n {
                    // SUBWF computes f - W; the IR's `sub a, b` is `a - b`,
                    // so `a` must be `f` and `b` must go into `W` first.
                    self.emit_load_w(&b.b, i, false);
                    // Byte 0 of add/sub is a plain ADDWF/SUBWF; every byte
                    // past it must fold in the carry/borrow from the
                    // previous byte via ADDWFC/SUBFWB. and/or/xor apply
                    // independently per byte and never use the carry form.
                    let carry = i > 0 && matches!(b.op, ir::BinOp::Add | ir::BinOp::Sub);
                    let mne = match (b.op, carry) {
                        (ir::BinOp::Add, false) => "ADDWF",
                        (ir::BinOp::Add, true) => "ADDWFC",
                        (ir::BinOp::Sub, false) => "SUBWF",
                        (ir::BinOp::Sub, true) => "SUBFWB",
                        (ir::BinOp::And, _) => "ANDWF",
                        (ir::BinOp::Or, _) => "IORWF",
                        (ir::BinOp::Xor, _) => "XORWF",
                        (other, _) => {
                            panic!("isel-pic18: Bin op {other:?} not yet implemented (Task 6+)")
                        }
                    };
                    let (aacc, af) = self.operand(av + u16::from(i));
                    let abank = if aacc == 0 { "A" } else { "B" };
                    self.emit(format!("    {mne} 0x{af:03X},W,{abank}"));
                    self.emit_w_store(dst + u16::from(i));
                }
            }
            Inst::Icmp(c) => {
                let n = c.ty.bytes();
                assert!(
                    n == 1 || n == 2 || n == 4,
                    "isel-pic18: only i8/i16/i32 Icmp implemented so far (n={n})"
                );
                if n == 1 {
                    self.emit_icmp_byte(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
                } else if n == 2 {
                    self.emit_icmp_i16(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
                } else {
                    self.emit_icmp_i32(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
                }
            }
            Inst::Zext(z) => {
                // `val_addr` maps `Val::Const(k)` to a RAM address, not a literal:
                // a constant source would read the byte at that address. The guard
                // keeps a future const-source producer panicking instead of
                // silently miscompiling.
                assert!(
                    !matches!(z.val, Val::Const(_)),
                    "isel-pic18: const source Zext not yet supported"
                );
                // Mirrors the `isel` narrowing guard: a `to.bytes() < from.bytes()`
                // `Zext` would copy past the destination into the next local.
                // Equal widths (`zext i1 to i8` reports 1 byte each: an `icmp`
                // result holds exactly 0/1) are legal zexts and stay accepted; the
                // high-byte loop simply does not execute when widths match.
                // Without the guard a malformed narrowing corrupts the neighbor
                // instead of panicking.
                assert!(
                    z.to.bytes() >= z.from.bytes(),
                    "isel-pic18: zext must not narrow"
                );
                let src = self.val_addr(&z.val).direct();
                let dst = self.slot_addr(self.cur_func, &z.dst).direct();
                for i in 0..z.from.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
                // A zero high byte needs no W staging: one CLRF word
                // where the pair costs two. CLRF sets Z, which is dead
                // here: this lowering reads no flags, and every lowering
                // sets its own flags before reading them.
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit_banked("CLRF", dst + u16::from(i), "");
                }
            }
            Inst::IntToPtr(p) => {
                // A runtime integer address becoming a pointer VALUE: copy
                // the two address bytes into the dst slot, which iselcore
                // seeded as an indirect pointer (`Base::Slot(dst, true)`).
                // Equal-width i16 -> i16, like a zext, but the dst is an
                // ADDRESS (derefs through FSR0/INDF0 per ADR-009).
                assert_eq!(
                    p.from, p.to,
                    "isel-pic18: inttoptr must keep the byte width (i16 -> ptr)"
                );
                assert!(
                    !matches!(p.val, Val::Const(_)),
                    "isel-pic18: const source IntToPtr not yet supported"
                );
                let src = self.val_addr(&p.val).direct();
                let dst = self.slot_addr(self.cur_func, &p.dst).direct();
                for i in 0..p.from.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
            }
            Inst::Freeze(f) => {
                // freeze is a no-op in the backend: copy `val` byte-for-byte
                // into the dst slot (same shape as `Inst::Zext` at equal
                // width, mirroring `isel`'s own Freeze arm).
                let src = self.val_addr(&f.val).direct();
                let dst = self.slot_addr(self.cur_func, &f.dst).direct();
                for i in 0..f.ty.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
            }
            Inst::VaStart(v) => {
                // The va list slot holds the ADDRESS of the current
                // argument in the `__va` region. va_start stores the
                // region base; a forwarded list (vprintf receiving
                // printf's `ap`) is a plain ptr param and needs none.
                let list = self.slot_addr(self.cur_func, &v.list).direct();
                let va_base = self
                    .addrs
                    .get(&ssa_key(self.cur_func, "__va"))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "isel-pic18: va_start in non-variadic context {} (no __va region)",
                            self.cur_func
                        )
                    });
                self.emit(format!("    MOVLW 0x{:02X}", (va_base & 0xFF) as u8));
                let (la, lf) = self.operand(list);
                self.emit(format!(
                    "    MOVWF 0x{lf:03X},{}",
                    if la == 0 { "A" } else { "B" }
                ));
                self.emit(format!("    MOVLW 0x{:02X}", ((va_base >> 8) & 0xFF) as u8));
            }
            Inst::VaArg(v) => {
                let da = self.slot_addr(self.cur_func, &v.dst).direct();
                let list = self.slot_addr(self.cur_func, &v.ptr).direct();
                for i in 0..v.ty.bytes() {
                    self.emit_fsr0_indirect_slot(list, 0, &[], i);
                    self.emit_copy_byte(0xFEF, da + u16::from(i)); // INDF0
                }
                for _ in 0..v.ty.bytes() {
                    let (la, lf) = self.operand(list);
                    self.emit(format!(
                        "    INCF 0x{lf:03X},F,{}",
                        if la == 0 { "A" } else { "B" }
                    ));
                    self.emit("    BTFSC 0xFD8,2,A".to_string()); // STATUS Z
                    let (ha, hf) = self.operand(list + 1);
                    self.emit(format!(
                        "    INCF 0x{hf:03X},F,{}",
                        if ha == 0 { "A" } else { "B" }
                    ));
                }
            }
            Inst::Sext(s) => {
                // Same const-source hazard as `Inst::Zext`, see its comment.
                assert!(
                    !matches!(s.val, Val::Const(_)),
                    "isel-pic18: const source Sext not yet supported"
                );
                // Mirrors the `isel` sext bounds guard: without it, `to.bytes() <=
                // `from.bytes()` sign-fills the wrong count while still copying the
                // full source width past the destination.
                // `i1 -> iN` holds exactly 0/1, so a plain copy is the sext and stays
                // accepted. Panics only on equal-or-narrowing widths that overrun
                // the destination.
                assert!(
                    s.to.bytes() >= s.from.bytes(),
                    "isel-pic18: sext must not narrow"
                );
                let src = self.val_addr(&s.val).direct();
                let dst = self.slot_addr(self.cur_func, &s.dst).direct();
                for i in 0..s.from.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
                // An i1 holds exactly 0/1, so widening zero-fills the
                // high bytes: one CLRF word each, same Z-dead argument
                // as the `Zext` fill above.
                if s.from == Ty::I1 && s.to.bytes() > s.from.bytes() {
                    for i in s.from.bytes()..s.to.bytes() {
                        self.emit_banked("CLRF", dst + u16::from(i), "");
                    }
                    // the i1 widening already zero-filled: skip the
                    // sign-fill loop below
                    return;
                }
                // The sign-fill byte(s) must reflect the SOURCE's actual
                // sign bit (bit 7 of its highest byte) at the time this
                // cast runs: not an assumption. `MOVLW 0x00` first, then
                // `BTFSC sign_byte,7` conditionally overwrites `W` with
                // `MOVLW 0xFF` only when that bit is set, so every high
                // byte gets the same, correctly-derived fill value.
                let sign_byte = src + u16::from(s.from.bytes()) - 1;
                for i in s.from.bytes()..s.to.bytes() {
                    self.emit("    MOVLW 0x00".to_string());
                    let (a, f) = self.operand(sign_byte);
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    BTFSC 0x{f:03X},7,{bank}"));
                    self.emit("    MOVLW 0xFF".to_string());
                    let (da, df) = self.operand(dst + u16::from(i));
                    let dbank = if da == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{df:03X},{dbank}"));
                }
            }
            Inst::Trunc(t) => {
                // Same const-source hazard as `Inst::Zext`, see its comment.
                assert!(
                    !matches!(t.val, Val::Const(_)),
                    "isel-pic18: const source Trunc not yet supported"
                );
                // `Ty::I1` and `Ty::I8` are both 1 byte, so byte widths alone
                // do not separate `trunc i8 -> i1` (narrowing) from a
                // non-narrowing trunc. Mirrors `isel` ("isel: trunc must
                // narrow").
                assert!(
                    t.from.bytes() > t.to.bytes() || (t.to == Ty::I1 && t.from != Ty::I1),
                    "isel-pic18: trunc must narrow (to must be strictly smaller than from)"
                );
                let src = self.val_addr(&t.val).direct();
                let dst = self.slot_addr(self.cur_func, &t.dst).direct();
                for i in 0..t.to.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
                if t.to == Ty::I1 {
                    // Every `i1` consumer tests the whole byte for nonzero,
                    // so high bits must be cleared: `0x02` is false.
                    self.emit("    MOVLW 0x01".to_string());
                    let (a, f) = self.operand(dst);
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    ANDWF 0x{f:03X},F,{bank}"));
                }
            }
            Inst::Select(s) => {
                if s.ptr && matches!((&s.a, &s.b), (Val::Const(_), Val::Const(_))) {
                    // A pointer select over two runtime address LITERALS
                    // (the HAL's `pir_reg_addr(d)` arms): the selected arm's
                    // address bytes must land in the dst slot, which iselcore
                    // seeded as an indirect pointer (`Base::Slot(dst, true)`).
                    // The two-byte value select below is exactly the
                    // materialization; the existing arms handle `Const` arms
                    // via `emit_move_val_to_slot`'s MOVLW path.
                } else if s.ptr && self.select_is_seeded(&s.dst) {
                    // A pointer select whose arms are runtime address VALUES
                    // that do not fold (distinct globals, a global vs a
                    // runtime slot, two runtime slots,): iselcore
                    // seeded the dst as an indirect slot, so the selected
                    // arm's address bytes must land in it. The two-byte
                    // value select below materializes them. (epic-cc#147)
                } else if s.ptr {
                    // A pointer-typed select folded by iselcore into the
                    // resolved map (a GEP-chain select): emits nothing, every
                    // load/store through it lowers via the fold. PIC18 has
                    // no const-arm fold, so this is the GEP-arm shape.
                    assert!(
                        !matches!(s.cond, Val::Const(_)),
                        "isel-pic18: const cond pointer Select not yet supported"
                    );
                }
                // `a`/`b` travel via `emit_move_val_to_slot`, which handles `Const`
                // and never branches on a flag, so neither arm needs a guard. A seeded
                // select holds an address value and uses `emit_move_addr_to_slot`.
                // `cond` loads via `emit_load_w` then tests `BZ`, which reads the Z
                // flag the load sets. Only `MOVF` sets Z; `MOVLW` leaves a stale flag
                // that selects the wrong side, so a `Const` cond panics. (epic-cc#147)
                let seeded = s.ptr && self.select_is_seeded(&s.dst);
                if !s.ptr || matches!((&s.a, &s.b), (Val::Const(_), Val::Const(_))) || seeded {
                    assert!(
                        !matches!(s.cond, Val::Const(_)),
                        "isel-pic18: const cond Select not yet supported"
                    );
                    let dst = self.slot_addr(self.cur_func, &s.dst).direct();
                    let addr_value = seeded;
                    let l_else = self.fresh_label();
                    let l_end = self.fresh_label();
                    self.emit_load_w(&s.cond, 0, true);
                    self.emit(format!("    BZ {l_else}")); // cond byte == 0 -> else
                    if addr_value {
                        self.emit_move_addr_to_slot(&s.a, dst);
                    } else {
                        self.emit_move_val_to_slot(&s.a, s.ty, dst);
                    }
                    self.emit(format!("    BRA {l_end}"));
                    self.emit_label(&l_else);
                    if addr_value {
                        self.emit_move_addr_to_slot(&s.b, dst);
                    } else {
                        self.emit_move_val_to_slot(&s.b, s.ty, dst);
                    }
                    self.emit_label(&l_end);
                }
            }
            Inst::Call(c) => {
                if !c.callees.is_empty() {
                    self.emit_indirect_call(&c.dst, c.ty, &c.func, &c.args, &c.callees);
                } else if !self.is_function(&c.func) {
                    // An indirect call site (numeric `func`, the SSA
                    // register) whose candidate list is empty cannot be a
                    // direct call: the target is a runtime value the
                    // compiler could not resolve (an opaque store into an
                    // ISR-visible global,). Emit the
                    // deterministic trap loop rather than panic on the
                    // register name or silently call nothing. (epic-cc#137)
                    let l_trap = self.fresh_label();
                    self.emit_label(&l_trap);
                    self.emit(format!("    BRA {l_trap}"));
                } else {
                    self.emit_call_args(&c.func, &c.args);
                    self.emit(format!("    CALL {}", c.func));
                    // A `CALL` return joins like a label: the callee ran its
                    // own `MOVLB` sequence. With the exit-bank map the join
                    // is precise: a callee whose every return path ends at
                    // one bank leaves that bank live, so the tracked value
                    // carries; an unknown exit clears. FSR0 has no such
                    // contract: the callee used it for its own pointer
                    // accesses (epic-cc#472). (epic-cc#495)
                    self.bsr = self.exit_banks.get(&c.func).copied().flatten();
                    self.fsr0_holds = None;
                    if let Some(d) = &c.dst {
                        let ty = c.ty.expect("isel-pic18: valued call must carry a type");
                        let dst = self.slot_addr(self.cur_func, d).direct();
                        for i in 0..ty.bytes() {
                            self.emit_copy_byte(self.retval_lo + u16::from(i), dst + u16::from(i));
                        }
                    }
                }
            }

            Inst::Alloca(_) | Inst::Gep(_) => {
                // Virtual: Alloca's slot comes from `alloc`'s layout and
                // Gep's result is folded away by `resolve_pointers`
                // before codegen ever runs; see
                // ADR-009. Neither emits
                // anything of its own.
            }
            Inst::Memcpy(mc) => match &mc.len {
                ir::MemLen::Const(n) => {
                    let n = *n;
                    // A staged run draining mid-copy would invalidate the
                    // setup decisions byte 0 makes; drain first (the
                    // epic-cc#486 review's drain-before-decision rule).
                    self.flush_copies();
                    if n >= 1 {
                        // Byte 0's setups resolve the copy's kinds once;
                        // later bytes walk from those positions instead
                        // of re-seeding per byte. (epic-cc#492)
                        let (src0, dst0) = self.emit_memcpy_setups(mc, 0);
                        // A long const-source copy into a direct slot runs
                        // as one counted loop: the byte-0 setup already
                        // seeded TBLPTR, so only the destination pointer
                        // and the count are needed (epic-cc#504).
                        if matches!(src0, McSrc::Tblrd)
                            && self.const_copy_loops(n)
                            && matches!(dst0, Addr::Direct(_))
                        {
                            if let Addr::Direct(d) = dst0 {
                                self.emit_const_copy_loop(d, n);
                            }
                            return;
                        }
                        let walk = n >= 2
                            && match (src0, dst0) {
                                // Pure address math per byte, and #486
                                // staging owns the bodies: nothing to
                                // save by walking.
                                (McSrc::Direct(_), Addr::Direct(_)) => false,
                                _ => true,
                            };
                        self.emit_memcpy_body(n == 1, &src0, &dst0, walk);
                        for i in 1..n {
                            let last = i + 1 == n;
                            if walk {
                                // Advance byte-0's direct addresses; the
                                // walked pointers advance themselves, so
                                // each address is computed exactly once.
                                let s = match src0 {
                                    McSrc::Direct(a) => McSrc::Direct(a + u16::from(i)),
                                    s => s,
                                };
                                let d = match dst0 {
                                    Addr::Direct(d) => Addr::Direct(d + u16::from(i)),
                                    d => d,
                                };
                                self.emit_memcpy_body(last, &s, &d, true);
                            } else {
                                let (s, d) = self.emit_memcpy_setups(mc, i);
                                self.emit_memcpy_body(last, &s, &d, false);
                            }
                        }
                        if walk && matches!(dst0, Addr::Indirect) {
                            self.bump_fsr0_tracked_offset(n);
                        }
                    }
                }
                ir::MemLen::Reg(_) => {
                    panic!("isel-pic18: dynamic-length memcpy not yet supported (P3 scope)")
                }
            },
            Inst::Asm(a) => {
                self.emit("; --- asm start ---".to_string());
                let substituted = self.substitute_asm(&a.template, &a.operands);
                for line in substituted.split('\n') {
                    self.emit(line.to_string());
                }
                self.emit("; --- asm end ---".to_string());
            }
            other => panic!("isel-pic18: unsupported instruction for P2 (so far): {other:?}"),
        }
    }

    /// `dst = call %fp(args)` through a function pointer: an inline
    /// compare-and-call chain over the candidate set. Each candidate's two
    /// address bytes are compared against the fp value; on a match the args
    /// are copied into that candidate's param slots and the CALL runs, then
    /// control jumps to the shared retval copy. No candidate matches (a bogus
    /// or null fp, which a valid C program never reaches) falls into a
    /// deterministic trap loop rather than a silent wrong call. (epic-cc#73)
    fn emit_indirect_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        let l_done = self.fresh_label();
        let cand_exits: Vec<Option<u8>> = callees
            .iter()
            .map(|cand| self.exit_banks.get(cand).copied().flatten())
            .collect();
        for (cand, exit) in callees.iter().zip(cand_exits.iter()) {
            let l_next = self.fresh_label();
            // Compare the fp value's two bytes against the candidate's
            // address. MOVF sets Z; XORLW leaves it; BNZ skips on mismatch.
            self.emit_load_w(&Val::Reg(func.to_string()), 0, false);
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit(format!("    BNZ {l_next}"));
            self.emit_load_w(&Val::Reg(func.to_string()), 1, false);
            self.emit(format!("    XORLW HIGH({cand})"));
            self.emit(format!("    BNZ {l_next}"));
            // Matched: copy args into this candidate's slots and call it.
            self.emit_call_args(cand, args);
            self.emit(format!("    CALL {cand}"));
            // Same contract as the direct arm, per candidate: a proven
            // exit bank carries, an unknown one clears.
            self.bsr = *exit;
            self.fsr0_holds = None;
            self.emit(format!("    BRA {l_done}"));
            self.emit_label(&l_next);
        }
        // No candidate matched: deterministic trap.
        let l_trap = self.fresh_label();
        self.emit_label(&l_trap);
        self.emit(format!("    BRA {l_trap}"));
        self.emit_label(&l_done);
        // Only the candidate arms' BRAs reach the done label (the trap
        // loops), so the candidate exit meet is the label's true entry
        // bank. Restore it directly: the forward join cannot, because
        // the label's linear fall-through comes from the trap block,
        // whose bank is unknown. (epic-cc#495)
        if let Some(bank) = exit_bank(&cand_exits) {
            self.bsr = Some(bank);
        }
        if let Some(d) = dst {
            let t = ty.expect("isel-pic18: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                self.emit_copy_byte(self.retval_lo + u16::from(i), da + u16::from(i));
            }
        }
    }

    /// Computes `dst = (a <pred> b) ? 1 : 0` for one byte via a flag
    /// branch using standard condition codes (C=1 means no borrow).
    /// Shares the flag test with the 16-bit compare. A lone byte resolves
    /// equality directly to the predicate answer: true for `eq`/`uge`/`ule`/
    /// `sge`/`sle`, false for the strict ones. Routing equality uniformly
    /// to `l_false` inverts the non-strict predicates at equal inputs.
    fn emit_icmp_byte(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // `val_addr` maps `Val::Const(k)` to a RAM address, not a literal:
        // a constant LHS would compare against the byte at that address
        // instead of the literal. Same hazard as the `Bin` LHS arm;
        // panics until later lowering adds const-LHS canonicalization.
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        let l_true = self.fresh_label();
        let l_false = self.fresh_label();
        let l_done = self.fresh_label();
        let l_equal = if matches!(pred, "eq" | "uge" | "ule" | "sge" | "sle") {
            l_true.clone()
        } else {
            l_false.clone()
        };
        let pre = self.bool_result_preclear(&a, &b, dst, 1);
        if let Some(d) = pre {
            self.emit_banked("CLRF", d, "");
        }
        self.emit_cmp_branch(&a, &b, 0, pred, &l_true, &l_false, &l_equal);
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst, pre);
    }

    /// Computes the 16-bit predicate by comparing the high byte first with
    /// the predicate signedness: a difference there decides the result, as
    /// the sign lives in the top byte. Equal high bytes defer to the low
    /// byte compared unsigned. `eq`/`ne` bypass this shape: both bytes must
    /// match (`eq`) or either differ (`ne`), so they use the dedicated
    /// short-circuit instead of the ordering tie-break.
    fn emit_icmp_i16(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // Same const-LHS hazard as `emit_icmp_byte`: this is a separate
        // entry point (not routed through `emit_icmp_byte`), so it needs
        // its own guard rather than inheriting one transitively.
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        if pred == "eq" || pred == "ne" {
            self.emit_icmp_i16_eq_ne(a, b, pred, dst);
            return;
        }
        // The unsigned predicates go through the borrow chain, which has
        // one shared exit rather than a per-lane cascade (see
        // `emit_icmp_chain`). Signed ones keep the cascade: their
        // answer needs the top lanes' sign equality, which a single final
        // borrow does not carry. A rhs whose lane load writes STATUS,C
        // would break the chain's carried flag, so that shape keeps the
        // cascade too (epic-cc#621).
        if matches!(pred, "ult" | "uge" | "ugt" | "ule") && !self.load_w_writes_carry(&b) {
            self.emit_icmp_chain(a, b, pred, dst, 2, None);
            return;
        }
        let unsigned_tiebreak = match pred {
            "slt" => "ult",
            "sle" => "ule",
            "sgt" => "ugt",
            "sge" => "uge",
            other => other, // ult/ule/ugt/uge tie-break against themselves
        };
        let l_true = self.fresh_label();
        let l_false = self.fresh_label();
        let l_done = self.fresh_label();
        let l_check_low = self.fresh_label();
        let pre = self.bool_result_preclear(&a, &b, dst, 2);
        if let Some(d) = pre {
            self.emit_banked("CLRF", d, "");
        }

        // High byte, `pred`'s own signedness. Equal high bytes never
        // decide the outcome by themselves (a lower byte could still flip
        // the overall order either way) so "equal" always defers to the
        // low-byte tie-break, regardless of predicate.
        self.emit_cmp_branch(&a, &b, 1, pred, &l_true, &l_false, &l_check_low);
        self.emit_label(&l_check_low);
        // Low byte, unsigned tie-break. Here "equal" means the two
        // 16-bit values are fully identical, so: unlike the high byte
        // it DOES have a final answer: true for the non-strict tie-break
        // predicates (`ule`/`uge`), false for the strict ones
        // (`ult`/`ugt`).
        let l_low_equal = if matches!(unsigned_tiebreak, "ule" | "uge") {
            l_true.clone()
        } else {
            l_false.clone()
        };
        self.emit_cmp_branch(
            &a,
            &b,
            0,
            unsigned_tiebreak,
            &l_true,
            &l_false,
            &l_low_equal,
        );
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst, pre);
    }

    /// Unsigned ordering compare as one low-to-high borrow chain with a
    /// single exit. `a - b` propagates the borrow upward, so the final
    /// carry is `a >= b` for every width; the chain never inspects a lane
    /// a higher lane could still decide, and it needs no per-lane exit.
    /// `pred` must be one of `ult`/`uge`/`ugt`/`ule`.
    ///
    /// `ugt`/`ule` are `uge`/`ult` with one extra borrow: injecting a
    /// borrow-in of 1 at lane 0 computes `a - b - 1`, so its carry is
    /// `a >= b + 1`, i.e. exactly `a > b`. The operands are never swapped:
    /// `a` must stay the memory operand, because a swapped constant on the
    /// left would resolve through `val_addr` to its RAM address instead of
    /// as a literal (the hazard every `Icmp` entry point asserts against).
    ///
    /// Costs `2n` words plus 1 for a borrow-in predicate and 3 for the
    /// exit, against the cascade's `2n + 3n`. `fuse` carries a consuming
    /// `BrCond`'s exit labels, when this compare's result has no other use.
    fn emit_icmp_chain(
        &mut self,
        a: Val,
        b: Val,
        pred: &str,
        dst: &str,
        bytes: u8,
        fuse: Option<(String, String)>,
    ) {
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        debug_assert!(matches!(pred, "ult" | "uge" | "ugt" | "ule"));
        // Both chains subtract `b` from `a`. The plain chain's C=1 means
        // `a >= b` (giving `uge` at C=1 and `ult` at C=0); the strict chain
        // injects a borrow-in of 1, computing `a - b - 1`, whose C=1 means
        // `a > b` (giving `ugt` at C=1 and `ule` at C=0).
        let borrow_in = matches!(pred, "ugt" | "ule");
        // Which side of the branch the two outcomes go to: for the
        // materializing path they are the true/false labels, for a fused
        // one they are the branch's own targets.
        let (l_true, l_false, l_done);
        match fuse {
            Some((t, f)) => {
                l_true = t;
                l_false = f;
                l_done = None;
            }
            None => {
                l_true = self.fresh_label();
                l_false = self.fresh_label();
                l_done = Some(self.fresh_label());
            }
        }
        // `l_ge` is the C=1 target and `l_lt` the C=0 one. The non-strict
        // chain's C=1 means `a >= b` and the strict chain's means `a > b`,
        // so the two "greater" predicates and the two "less" ones pair up
        // across the strictness split, not within it:
        //   uge (a>=b) -> C=1, ugt (a>b) -> C=1
        //   ult (a<b)  -> C=0, ule (a<=b) -> C=0
        let (l_ge, l_lt) = if matches!(pred, "uge" | "ugt") {
            (l_true.clone(), l_false.clone())
        } else {
            (l_false.clone(), l_true.clone())
        };
        // The materializing path writes a 0/1 byte only; a fused compare
        // leaves the slot alone entirely, so the preclear is skipped.
        let pre = if l_done.is_some() {
            self.bool_result_preclear(&a, &b, dst, bytes)
        } else {
            None
        };
        if let Some(d) = pre {
            self.emit_banked("CLRF", d, "");
        }
        if borrow_in {
            // C = 0 makes lane 0's SUBWFB subtract `b0 + 1`.
            self.emit("    BCF 0xFD8,0,A".to_string());
        }
        for i in 0..bytes {
            self.emit_load_w(&b, i, false);
            let av = self.val_addr(&a).direct() + u16::from(i);
            let (acc, af) = self.operand(av);
            let bank = if acc == 0 { "A" } else { "B" };
            // Lane 0 uses SUBWF for the plain chain (which sets C
            // outright) and SUBWFB for the borrow-in one (which consumes
            // the carry cleared above, so it cannot be the plain form).
            let mne = if i == 0 && !borrow_in {
                "SUBWF"
            } else {
                "SUBWFB"
            };
            self.emit(format!("    {mne} 0x{af:03X},W,{bank}"));
        }
        // C=1 is always the `>=`/`>` side, whichever chain ran.
        self.emit(format!("    BNC {l_lt}"));
        self.emit(format!("    BRA {l_ge}"));
        if let Some(done) = l_done {
            self.emit_materialize_bool(&l_true, &l_false, &done, dst, pre);
        }
    }

    /// `eq`/`ne` for multi-byte values: true (for `eq`) only when every
    /// byte matches; `ne` is the mirror. Per-byte checks through
    /// `emit_cmp_flags` (`MOVF` for a zero literal lane, `SUBWF` otherwise)
    /// then `BNZ`, independent of the signed/unsigned tie-break machinery
    /// used for the eight ordering predicates: a partial match (some byte
    /// equal, another different) is decisive here in a way it never is for
    /// `slt`/`ult`/etc.
    fn emit_icmp_i16_eq_ne(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        self.emit_icmp_eq_ne(a, b, pred, dst, 2, None);
    }

    /// `fuse` carries the two exit labels of a consuming `BrCond` in this
    /// function, when this compare's result has no other use. With it, the
    /// per-byte exits branch straight to the branch targets and no 0/1
    /// byte is materialized (see `emit_fused_branch`).
    fn emit_icmp_eq_ne(
        &mut self,
        a: Val,
        b: Val,
        pred: &str,
        dst: &str,
        bytes: u8,
        fuse: Option<(String, String)>,
    ) {
        let (l_true, l_false, l_done);
        match fuse {
            Some((t, f)) => {
                l_true = t;
                l_false = f;
                l_done = None;
            }
            None => {
                l_true = self.fresh_label();
                l_false = self.fresh_label();
                l_done = Some(self.fresh_label());
            }
        }
        let l_mismatch = if pred == "eq" { &l_false } else { &l_true };
        let pre = if l_done.is_some() {
            self.bool_result_preclear(&a, &b, dst, bytes)
        } else {
            None
        };
        if let Some(d) = pre {
            self.emit_banked("CLRF", d, "");
        }
        for offset in 0..bytes {
            self.emit_cmp_flags(&a, &b, offset, pred);
            self.emit(format!("    BNZ {l_mismatch}"));
        }
        // Every byte matched: `eq` is true, `ne` is false.
        let l_all_matched = if pred == "eq" { &l_true } else { &l_false };
        self.emit(format!("    BRA {l_all_matched}"));
        if let Some(done) = l_done {
            self.emit_materialize_bool(&l_true, &l_false, &done, dst, pre);
        }
    }

    /// `dst = (a <pred> b) ? 1: 0` for four bytes: compare the high byte
    /// (offset 3) first with `pred`'s own signedness: if it differs,
    /// that alone decides the whole 32-bit result. Only when the high
    /// bytes are equal does the next byte get compared, and so on down to
    /// byte 0, always **unsigned** for the tie-breaks (same rule as
    /// `emit_icmp_i16`).
    ///
    /// `eq`/`ne` dispatch to `emit_icmp_eq_ne` with `bytes = 4`.
    fn emit_icmp_i32(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        if pred == "eq" || pred == "ne" {
            self.emit_icmp_eq_ne(a, b, pred, dst, 4, None);
            return;
        }
        // Same routing as the 16-bit entry point: the unsigned predicates
        // use the shared-exit borrow chain at any width, the signed ones
        // keep the cascade (see `emit_icmp_i16`). A rhs lane load that
        // writes STATUS,C keeps the cascade as well.
        if matches!(pred, "ult" | "uge" | "ugt" | "ule") && !self.load_w_writes_carry(&b) {
            self.emit_icmp_chain(a, b, pred, dst, 4, None);
            return;
        }
        let unsigned_tiebreak = match pred {
            "slt" => "ult",
            "sle" => "ule",
            "sgt" => "ugt",
            "sge" => "uge",
            other => other, // ult/ule/ugt/uge tie-break against themselves
        };
        let l_true = self.fresh_label();
        let l_false = self.fresh_label();
        let l_done = self.fresh_label();

        // Compare bytes 3 (the sign byte, `pred`'s own signedness), then
        // 2 and 1 as unsigned tie-breaks that defer to the next lower
        // byte on equality, then byte 0 with the final answer.
        let l_check_b2 = self.fresh_label();
        let l_check_b1 = self.fresh_label();
        let l_check_b0 = self.fresh_label();
        let pre = self.bool_result_preclear(&a, &b, dst, 4);
        if let Some(d) = pre {
            self.emit_banked("CLRF", d, "");
        }

        self.emit_cmp_branch(&a, &b, 3, pred, &l_true, &l_false, &l_check_b2);
        self.emit_label(&l_check_b2);
        self.emit_cmp_branch(&a, &b, 2, unsigned_tiebreak, &l_true, &l_false, &l_check_b1);
        self.emit_label(&l_check_b1);
        self.emit_cmp_branch(&a, &b, 1, unsigned_tiebreak, &l_true, &l_false, &l_check_b0);
        self.emit_label(&l_check_b0);
        // Byte 0 equality is the full-value equality: final answer for the
        // non-strict tie-break predicates (`ule`/`uge`), false for the
        // strict ones (`ult`/`ugt`).
        let l_low_equal = if matches!(unsigned_tiebreak, "ule" | "uge") {
            l_true.clone()
        } else {
            l_false.clone()
        };
        self.emit_cmp_branch(
            &a,
            &b,
            0,
            unsigned_tiebreak,
            &l_true,
            &l_false,
            &l_low_equal,
        );
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst, pre);
    }

    /// Compares `a`'s byte at `byte_offset` against `b`'s, leaving the
    /// STATUS flags the predicate's branches read. `eq`/`ne` consume only
    /// Z, so a zero literal byte skips the subtract: `MOVF f,W` sets Z
    /// from `f` itself, one word where the staged pair costs two. The
    /// ordering predicates branch on C or N/OV, which `MOVF` leaves
    /// alone, so the gate is "reads only Z", not "the literal is zero".
    fn emit_cmp_flags(&mut self, a: &Val, b: &Val, byte_offset: u8, pred: &str) {
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        let av = self.val_addr(a).direct() + u16::from(byte_offset);
        if matches!(pred, "eq" | "ne") {
            if let Val::Const(k) = b {
                if (k >> (u32::from(byte_offset) * 8)) & 0xFF == 0 {
                    let (acc, af) = self.operand(av);
                    let bank = if acc == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVF 0x{af:03X},W,{bank}")); // Z = (a == 0)
                    return;
                }
            }
        }
        self.emit_load_w(b, byte_offset, false);
        let (acc, af) = self.operand(av);
        let bank = if acc == 0 { "A" } else { "B" };
        self.emit(format!("    SUBWF 0x{af:03X},W,{bank}")); // W = a - b
    }

    /// Branches a byte compare three ways: `l_true` when the predicate holds,
    /// `l_false` on a decisive mismatch, `l_equal` on equal bytes. Equality
    /// stays ambiguous by design: the caller binds `l_equal` to defer (the
    /// high-byte step) or to answer (single bytes and low-byte tie-breaks).
    /// `eq`/`ne` skip the split: one byte decides them, so only `l_true` and
    /// `l_false` apply.
    fn emit_cmp_branch(
        &mut self,
        a: &Val,
        b: &Val,
        byte_offset: u8,
        pred: &str,
        l_true: &str,
        l_false: &str,
        l_equal: &str,
    ) {
        self.emit_cmp_flags(a, b, byte_offset, pred);

        match pred {
            "eq" => {
                self.emit(format!("    BZ {l_true}"));
                self.emit(format!("    BRA {l_false}"));
            }
            "ne" => {
                self.emit(format!("    BNZ {l_true}"));
                self.emit(format!("    BRA {l_false}"));
            }
            "ult" => {
                self.emit(format!("    BNC {l_true}")); // C=0: a<b, definite
                self.emit(format!("    BZ {l_equal}"));
                self.emit(format!("    BRA {l_false}"));
            }
            "uge" => {
                self.emit(format!("    BNC {l_false}")); // C=0: a<b, definite
                self.emit(format!("    BZ {l_equal}"));
                self.emit(format!("    BRA {l_true}")); // C=1,Z=0: a>b, definite
            }
            "ugt" => {
                self.emit(format!("    BNC {l_false}")); // C=0: a<b, definite
                self.emit(format!("    BZ {l_equal}"));
                self.emit(format!("    BRA {l_true}")); // C=1,Z=0: a>b, definite
            }
            "ule" => {
                self.emit(format!("    BNC {l_true}")); // C=0: a<b, definite
                self.emit(format!("    BZ {l_equal}"));
                self.emit(format!("    BRA {l_false}")); // C=1,Z=0: a>b, definite
            }
            "slt" => {
                // N != OV: true if (N set and OV clear) or (N clear and OV set).
                self.emit(format!("    BZ {l_equal}"));
                let l_check_ov = self.fresh_label();
                self.emit(format!("    BN {l_check_ov}"));
                self.emit(format!("    BOV {l_true}")); // N=0: true only if OV=1
                self.emit(format!("    BRA {l_false}"));
                self.emit_label(&l_check_ov);
                self.emit(format!("    BNOV {l_true}")); // N=1: true only if OV=0
                self.emit(format!("    BRA {l_false}"));
            }
            "sge" => {
                // N == OV: true if (N set and OV set) or (N clear and OV clear).
                self.emit(format!("    BZ {l_equal}"));
                let l_check_ov = self.fresh_label();
                self.emit(format!("    BN {l_check_ov}"));
                self.emit(format!("    BNOV {l_true}")); // N=0: true only if OV=0
                self.emit(format!("    BRA {l_false}"));
                self.emit_label(&l_check_ov);
                self.emit(format!("    BOV {l_true}")); // N=1: true only if OV=1
                self.emit(format!("    BRA {l_false}"));
            }
            "sgt" => {
                // Z=0 AND N==OV.
                self.emit(format!("    BZ {l_equal}"));
                let l_check_ov = self.fresh_label();
                self.emit(format!("    BN {l_check_ov}"));
                self.emit(format!("    BNOV {l_true}"));
                self.emit(format!("    BRA {l_false}"));
                self.emit_label(&l_check_ov);
                self.emit(format!("    BOV {l_true}"));
                self.emit(format!("    BRA {l_false}"));
            }
            "sle" => {
                // Z=1 OR N!=OV.
                self.emit(format!("    BZ {l_equal}"));
                let l_check_ov = self.fresh_label();
                self.emit(format!("    BN {l_check_ov}"));
                self.emit(format!("    BOV {l_true}"));
                self.emit(format!("    BRA {l_false}"));
                self.emit_label(&l_check_ov);
                self.emit(format!("    BNOV {l_true}"));
                self.emit(format!("    BRA {l_false}"));
            }
            other => panic!(
                "isel-pic18: icmp predicate {other} unreachable (ir::parse validates the 10-entry set)"
            ),
        }
    }

    /// Common `l_false: MOVLW 0x00 / l_true: MOVLW 0x01` materialization
    /// shared by `emit_icmp_byte`, `emit_icmp_i16`, and
    /// `emit_icmp_i16_eq_ne`: the only difference between the three is
    /// how they arrive at `l_true`/`l_false`.
    ///
    /// `precleared` is the result slot a caller has already zeroed before
    /// the compare (see `bool_result_preclear`). Then the compare's own
    /// branches already land on opposite sides of a single `INCF`, so the
    /// whole `MOVLW`/`BRA`/`MOVLW`/`MOVWF` diamond collapses into it.
    fn emit_materialize_bool(
        &mut self,
        l_true: &str,
        l_false: &str,
        l_done: &str,
        dst: &str,
        precleared: Option<u16>,
    ) {
        if let Some(d) = precleared {
            self.emit_label(l_true);
            self.emit_banked("INCF", d, ",F");
            self.emit_label(l_false);
            return;
        }
        self.emit_label(l_false);
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    BRA {l_done}"));
        self.emit_label(l_true);
        self.emit("    MOVLW 0x01".to_string());
        self.emit_label(l_done);
        let d = self.slot_addr(self.cur_func, dst).direct();
        let (da, df) = self.operand(d);
        let dbank = if da == 0 { "A" } else { "B" };
        self.emit(format!("    MOVWF 0x{df:03X},{dbank}"));
    }

    /// The result slot to zero before the compare, or `None` when zeroing
    /// it is not provably safe and the caller must keep the join form.
    ///
    /// Zeroing first lets the compare's branches select between "leave it
    /// zero" and a single `INCF`, which is what removes the diamond. It is
    /// only sound when `dst` overlaps nothing the compare reads: the
    /// layout may place a definition over an operand's storage, and
    /// clearing such a slot would destroy the operand before it is ever
    /// compared.
    fn bool_result_preclear(&self, a: &Val, b: &Val, dst: &str, bytes: u8) -> Option<u16> {
        /// The bytes `v` is read from, or `None` when that cannot be
        /// pinned to one base: a literal and a function label read no
        /// memory at all, while a GEP-derived pointer is read through its
        /// own address computation rather than the `val_addr` slot.
        fn read_base(gen: &Gen, v: &Val, b_operand: bool) -> Option<Option<u16>> {
            match v {
                Val::Const(_) => Some(None),
                Val::Global(g) if gen.is_function(g) => Some(None),
                Val::Reg(r) if b_operand => {
                    // Only B reaches `emit_load_w`, which reads a
                    // `resolved` value through its own address computation
                    // rather than the `val_addr` slot. A goes through
                    // `val_addr` whatever `resolved` says, so modelling it
                    // as the pointed-to base would be wrong. Model B for the
                    // one shape where the read stays inside a single slot
                    // plus a constant offset, and decline the rest so the
                    // join form is kept. (epic-cc#519)
                    let Some((base, k, terms)) = gen
                        .resolved
                        .get(&iselcore::ssa_key(gen.cur_func, r))
                        .cloned()
                    else {
                        return Some(Some(gen.val_addr(v).direct()));
                    };
                    if !terms.is_empty() {
                        // A dynamic term adds an index register, so the
                        // read is not bounded by this slot's own bytes.
                        return None;
                    }
                    // `emit_load_w`'s resolved arm asserts `holds_addr` and
                    // panics otherwise, so only an indirect (sret) slot or a
                    // pointer param is a read at all.
                    let iselcore::Base::Slot(sname, indirect) = &base else {
                        return None;
                    };
                    let holds_addr = *indirect
                        || gen
                            .m
                            .funcs
                            .iter()
                            .find(|f| f.name == gen.cur_func)
                            .map(|f| f.params.iter().any(|pp| pp.name == *sname && pp.ptr))
                            .unwrap_or(false);
                    if !holds_addr {
                        return None;
                    }
                    // The read is the SLOT's own bytes: `emit_load_w` emits
                    // `MOVF sa,W` and folds the constant offset in W with
                    // `ADDLW`, so `k` never shifts the memory address.
                    // (epic-cc#519 review)
                    let _ = k;
                    Some(Some(gen.slot_addr(gen.cur_func, sname).direct()))
                }
                other => Some(Some(gen.val_addr(other).direct())),
            }
        }
        let d = self.slot_addr(self.cur_func, dst).direct();
        let mut read = Vec::with_capacity(bytes as usize * 2);
        for (v, must_read) in [(a, true), (b, false)] {
            // `must_read` is true for A and false for B; B is the
            // operand `emit_load_w` lowers, so it is the only one whose
            // read bytes may be modelled through its address computation.
            match read_base(self, v, !must_read) {
                Some(Some(base)) => {
                    for i in 0..u16::from(bytes) {
                        read.push(base + i);
                    }
                }
                // A literal `a` never reaches here (every entry point
                // asserts it), and neither literal form is a byte slot the
                // clear could disturb.
                Some(None) if !must_read => {}
                _ => return None,
            }
        }
        (!read.contains(&d)).then_some(d)
    }

    /// Whether `emit_load_w(v, i)` writes STATUS,C for any lane in
    /// `0..bytes`.
    ///
    /// The shared-exit borrow chain (epic-cc#621) holds `C` live from one
    /// lane to the next, and across the lane-0 borrow-in seed, so a lane
    /// load that touches `C` corrupts it. The one `emit_load_w` arm that
    /// writes `C` is the resolved-GEP address materialization: `ADDLW` for
    /// the constant offset, `ADDWF` for a dynamic term, and the
    /// `BTFSC`/`ADDLW` carry fill on lane 1. Every other arm is `MOVF`,
    /// `MOVLW` or `MOVLB`, none of which touch `C`. A compare whose rhs
    /// load writes `C` falls back to the per-lane cascade, which consumes
    /// `C` from its own `SUBWF` immediately after each load and so is
    /// immune (epic-cc#519's `read_base` models the same arm).
    fn load_w_writes_carry(&self, v: &Val) -> bool {
        let Val::Reg(r) = v else {
            return false;
        };
        let Some((base, k, terms)) = self.resolved.get(&iselcore::ssa_key(self.cur_func, r)) else {
            return false;
        };
        // A literal base panics inside `emit_load_w` rather than emitting
        // anything, so it is not a clobber source.
        let iselcore::Base::Slot(sname, indirect) = base else {
            return false;
        };
        let holds_addr = *indirect
            || self
                .m
                .funcs
                .iter()
                .find(|f| f.name == self.cur_func)
                .map(|f| f.params.iter().any(|pp| pp.name == *sname && pp.ptr))
                .unwrap_or(false);
        if !holds_addr {
            // `emit_load_w` asserts here; nothing is emitted.
            return false;
        }
        // Constant offset only, or a dynamic term: both reach a carry-
        // writing add.
        *k != 0 || !terms.is_empty()
    }

    /// Load byte `offset` of any `Val` into `W`: a constant via `MOVLW`
    /// (shifting the literal right by `offset*8` bytes first), a
    /// register/global via `MOVF ...,W` at the resolved address plus
    /// `offset` (which needs the access bit, same as any other
    /// `W`-routing instruction).
    ///
    /// The resolved-GEP arm can write STATUS,C (see
    /// `load_w_writes_carry`); every other arm leaves it alone.
    ///
    /// `need_z` is forwarded to `emit_w_load` for the plain slot arms: a
    /// caller whose next instruction consumes the Z this load sets (the
    /// `BrCond`/`Select`/switch selectors) passes `true` so the reload is
    /// never elided. A caller feeding an ALU op passes `false`, since
    /// that op sets its own flags.
    fn emit_load_w(&mut self, v: &Val, offset: u8, need_z: bool) {
        match v {
            Val::Const(k) => {
                let byte = ((*k >> (u32::from(offset) * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{byte:02X}"));
            }
            Val::Reg(r) => {
                if false {
                    let keys: Vec<_> = self
                        .resolved
                        .keys()
                        .filter(|k| k.starts_with(self.cur_func))
                        .collect();
                    panic!("debug keys for {}: {:?}", self.cur_func, keys);
                }
                if let Some((base, k, terms)) = self
                    .resolved
                    .get(&iselcore::ssa_key(self.cur_func, r))
                    .cloned()
                {
                    // GEP pointer value materialization for returns and scalar
                    // pointer copies. Two base kinds hold runtime address
                    // bytes: a plain pointer param's slot and a
                    // runtime-address slot (an IntToPtr or const-arm select
                    // dst); both read as `base + k + terms`. Literal bases
                    // panic (their address is a link-time
                    // constant).
                    let sa = match &base {
                        iselcore::Base::Slot(sname, indirect) => {
                            let holds_addr = if *indirect {
                                true
                            } else {
                                self.m
                                    .funcs
                                    .iter()
                                    .find(|f| f.name == self.cur_func)
                                    .map(|f| f.params.iter().any(|pp| pp.name == *sname && pp.ptr))
                                    .unwrap_or(false)
                            };
                            assert!(
                                holds_addr,
                                "isel-pic18: cannot take the value of a GEP over {base:?}"
                            );
                            self.slot_addr(self.cur_func, sname).direct()
                        }
                        other => {
                            panic!("isel-pic18: cannot take the value of a GEP over {other:?}")
                        }
                    };
                    let adds_in_byte0 = k != 0 || !terms.is_empty();
                    assert!(
                        k == 0 || terms.is_empty(),
                        "isel-pic18: GEP with both a constant offset and dynamic terms loses the term's carry; not supported"
                    );
                    match terms.as_slice() {
                        [] => {
                            if offset == 0 {
                                let (a, f) = self.operand(sa);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                if k != 0 {
                                    self.emit(format!("    ADDLW 0x{k:02X}"));
                                }
                            } else {
                                let (a, f) = self.operand(sa + 1);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                if adds_in_byte0 {
                                    self.emit("    BTFSC 0xFD8,0,A".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                }
                            }
                            return;
                        }
                        [(1, reg)] => {
                            let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                            let (ra_a, ra_f) = self.operand(ra);
                            let ra_bank = if ra_a == 0 { "A" } else { "B" };
                            let ra1 = ra + 1;
                            let (ra1_a, ra1_f) = self.operand(ra1);
                            let ra1_bank = if ra1_a == 0 { "A" } else { "B" };
                            if offset == 0 {
                                let (a, f) = self.operand(sa);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                self.emit(format!("    ADDWF 0x{ra_f:03X},W,{ra_bank}"));
                            } else {
                                let (a, f) = self.operand(sa + 1);
                                let bank = if a == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
                                self.emit("    BTFSC 0xFD8,0,A".to_string());
                                self.emit("    ADDLW 0x01".to_string());
                                self.emit(format!("    ADDWF 0x{ra1_f:03X},W,{ra1_bank}"));
                            }
                            return;
                        }
                        _ => panic!("isel-pic18: multi-term GEP load with {terms:?} not supported"),
                    }
                }
                let addr = self.val_addr(v).direct() + u16::from(offset);
                self.emit_w_load(addr, need_z);
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    // A function's address is a link-time label literal:
                    // byte 0 = LOW(g), byte 1 = HIGH(g). (epic-cc#73)
                    let lit = if offset == 0 { "LOW" } else { "HIGH" };
                    self.emit(format!("    MOVLW {lit}({g})"));
                } else {
                    let addr = self.val_addr(v).direct() + u16::from(offset);
                    self.emit_w_load(addr, need_z);
                }
            }
        }
    }

    /// Copy `bytes` bytes from `src` (a slot address) into the fixed
    /// retval region (`retval_lo`). MOVFF-based; no access bit, no BSR.
    fn store_retval(&mut self, src: u16, bytes: u8) {
        for i in 0..bytes {
            self.emit_copy_byte(src + u16::from(i), self.retval_lo + u16::from(i));
        }
    }

    /// Two's-complement negate of a `bytes`-byte value in place: `COMF`
    /// every byte, then `INCF` the low byte, and each higher byte
    /// increments ONLY if the previous byte's `INCF` wrapped to zero (the
    /// carry propagates through the Z chain, `BTFSC STATUS,2` before each
    /// higher `INCF`, exactly PIC14's `neg16_in_place`/`neg32_in_place`).
    /// An unconditional `INCF` on every byte would turn `0xFFED` (-19) into
    /// `0x0113` instead of `0x0013` (19): the high byte must not advance
    fn neg_in_place(&mut self, addr: u16, bytes: u8) {
        for i in 0..bytes {
            let (a, f) = self.operand(addr + u16::from(i));
            self.emit(format!(
                "    COMF 0x{f:03X},F,{}",
                if a == 0 { "A" } else { "B" }
            ));
        }
        let (a0, f0) = self.operand(addr);
        self.emit(format!(
            "    INCF 0x{f0:03X},F,{}",
            if a0 == 0 { "A" } else { "B" }
        ));
        for i in 1..bytes {
            self.emit("    BTFSC 0xFD8,2,A".to_string()); // STATUS Z
            let (a, f) = self.operand(addr + u16::from(i));
            self.emit(format!(
                "    INCF 0x{f:03X},F,{}",
                if a == 0 { "A" } else { "B" }
            ));
        }
    }

    /// The hardware-multiply recipe for `bytes` = 1, 2, or 4: schoolbook
    /// partial products via `MULWF` (8x8 -> PRODH:PRODL, SFRs 0xFF4/0xFF3),
    /// the runtime routines headline. The result is the low `bytes` bytes of the product,
    /// written to the retval region:
    /// - 1 byte: one MULWF, result = PRODL.
    /// - 2 bytes: P00 (shift 0) and P01 + P10 (shift 8) contribute to the
    /// low 16 bits; P11 (shift 16) is dropped.
    fn emit_hw_mul(&mut self, name: &str, bytes: u8, scr: u16) {
        let a = self.slot_addr(name, "a").direct();
        let b = self.slot_addr(name, "b").direct();
        match bytes {
            1 => {
                let (aa, af) = self.operand(a);
                self.emit(format!(
                    "    MOVF 0x{af:03X},W,{}",
                    if aa == 0 { "A" } else { "B" }
                ));
                let (ba, bf) = self.operand(b);
                self.emit(format!(
                    "    MULWF 0x{bf:03X},{}",
                    if ba == 0 { "A" } else { "B" }
                ));
                self.store_retval(0xFF3, 1); // PRODL
                self.emit("    RETURN".to_string());
            }
            2 => {
                let (r0, r1) = (scr, scr + 1);
                let (aa, af) = self.operand(a);
                self.emit(format!(
                    "    MOVF 0x{af:03X},W,{}",
                    if aa == 0 { "A" } else { "B" }
                ));
                let (ba, bf) = self.operand(b);
                self.emit(format!(
                    "    MULWF 0x{bf:03X},{}",
                    if ba == 0 { "A" } else { "B" }
                )); // P00
                self.emit(format!("    MOVFF 0xFF3, 0x{r0:03X}"));
                self.emit(format!("    MOVFF 0xFF4, 0x{r1:03X}"));
                let (aa, af) = self.operand(a);
                self.emit(format!(
                    "    MOVF 0x{af:03X},W,{}",
                    if aa == 0 { "A" } else { "B" }
                ));
                let (b1a, b1f) = self.operand(b + 1);
                self.emit(format!(
                    "    MULWF 0x{b1f:03X},{}",
                    if b1a == 0 { "A" } else { "B" }
                )); // P01
                self.emit("    MOVF 0xFF3,W,A".to_string());
                let (r1a, r1f) = self.operand(r1);
                self.emit(format!(
                    "    ADDWF 0x{r1f:03X},F,{}",
                    if r1a == 0 { "A" } else { "B" }
                ));
                let (a1a, a1f) = self.operand(a + 1);
                self.emit(format!(
                    "    MOVF 0x{a1f:03X},W,{}",
                    if a1a == 0 { "A" } else { "B" }
                ));
                let (ba, bf) = self.operand(b);
                self.emit(format!(
                    "    MULWF 0x{bf:03X},{}",
                    if ba == 0 { "A" } else { "B" }
                )); // P10
                self.emit("    MOVF 0xFF3,W,A".to_string());
                let (r1a, r1f) = self.operand(r1);
                self.emit(format!(
                    "    ADDWF 0x{r1f:03X},F,{}",
                    if r1a == 0 { "A" } else { "B" }
                ));
                self.store_retval(r0, 2);
                self.emit("    RETURN".to_string());
            }
            4 => {
                for i in 0..4u16 {
                    let (sa, sf) = self.operand(scr + i);
                    self.emit(format!(
                        "    CLRF 0x{sf:03X},{}",
                        if sa == 0 { "A" } else { "B" }
                    ));
                }
                for j in 0..4u16 {
                    for i in 0..4u16 {
                        let off = i + j;
                        if off >= 4 {
                            continue; // lands at shift >= 32, dropped
                        }
                        let (aa, af) = self.operand(a + i);
                        self.emit(format!(
                            "    MOVF 0x{af:03X},W,{}",
                            if aa == 0 { "A" } else { "B" }
                        ));
                        let (ba, bf) = self.operand(b + j);
                        self.emit(format!(
                            "    MULWF 0x{bf:03X},{}",
                            if ba == 0 { "A" } else { "B" }
                        ));
                        self.emit("    MOVF 0xFF3,W,A".to_string()); // PRODL
                        let (oa, of) = self.operand(scr + off);
                        self.emit(format!(
                            "    ADDWF 0x{of:03X},F,{}",
                            if oa == 0 { "A" } else { "B" }
                        ));
                        self.emit("    MOVF 0xFF4,W,A".to_string()); // PRODH
                        let (o1a, o1f) = self.operand(scr + off + 1);
                        self.emit(format!(
                            "    ADDWFC 0x{o1f:03X},F,{}",
                            if o1a == 0 { "A" } else { "B" }
                        ));
                    }
                }
                self.store_retval(scr, 4);
                self.emit("    RETURN".to_string());
            }
            _ => unreachable!("isel-pic18: hw mul width"),
        }
    }

    /// Runs one restoring-division bit: shifts `num` left, accumulates the
    /// partial remainder, subtracts `den` with the borrow chain, and adds
    /// back on borrow. `cnt` counts `8*den_bytes` iterations. Real `BNC`/`BRA`
    /// branches free the frame from any single-bank constraint. Shared by the
    /// unsigned and signed wrappers. The u8 case pairs a 2-byte remainder
    /// with a 1-byte divisor (implicit high byte 0 via `MOVLW 0` folds).
    fn emit_divmod_loop(
        &mut self,
        num: u16,
        den: u16,
        rem_base: u16,
        cnt: u16,
        den_bytes: u8,
        rem_bytes: u8,
    ) {
        let l_loop = self.fresh_label();
        let l_restore = self.fresh_label();
        let l_next = self.fresh_label();
        for i in 0..u16::from(rem_bytes) {
            let (ra, rf) = self.operand(rem_base + i);
            self.emit(format!(
                "    CLRF 0x{rf:03X},{}",
                if ra == 0 { "A" } else { "B" }
            ));
        }
        self.emit(format!("    MOVLW 0x{:02X}", 8 * den_bytes));
        let (ca, cf) = self.operand(cnt);
        self.emit(format!(
            "    MOVWF 0x{cf:03X},{}",
            if ca == 0 { "A" } else { "B" }
        ));
        self.emit_label(&l_loop);
        self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
        for i in 0..u16::from(den_bytes) {
            let (na, nf) = self.operand(num + i);
            self.emit(format!(
                "    RLCF 0x{nf:03X},F,{}",
                if na == 0 { "A" } else { "B" }
            ));
        }
        for i in 0..u16::from(rem_bytes) {
            let (ra, rf) = self.operand(rem_base + i);
            self.emit(format!(
                "    RLCF 0x{rf:03X},F,{}",
                if ra == 0 { "A" } else { "B" }
            ));
        }
        // rem -= den: SUBWF then SUBFWB (borrow-in); beyond den_bytes the
        // divisor byte is implicitly 0, folded with MOVLW 0 + SUBFWB.
        for i in 0..u16::from(rem_bytes) {
            if i < u16::from(den_bytes) {
                let (da, df) = self.operand(den + i);
                self.emit(format!(
                    "    MOVF 0x{df:03X},W,{}",
                    if da == 0 { "A" } else { "B" }
                ));
            } else {
                self.emit("    MOVLW 0x00".to_string());
            }
            let (ra, rf) = self.operand(rem_base + i);
            let mne = if i == 0 { "SUBWF" } else { "SUBFWB" };
            self.emit(format!(
                "    {mne} 0x{rf:03X},F,{}",
                if ra == 0 { "A" } else { "B" }
            ));
        }
        // C after the last byte = (rem >= den): set the quotient bit or restore.
        self.emit(format!("    BNC {l_restore}"));
        let (na, nf) = self.operand(num);
        self.emit(format!(
            "    BSF 0x{nf:03X},0,{}",
            if na == 0 { "A" } else { "B" }
        ));
        self.emit(format!("    BRA {l_next}"));
        self.emit_label(&l_restore);
        // rem += den back (ADDWF, then ADDWFC for the carries).
        for i in 0..u16::from(rem_bytes) {
            if i < u16::from(den_bytes) {
                let (da, df) = self.operand(den + i);
                self.emit(format!(
                    "    MOVF 0x{df:03X},W,{}",
                    if da == 0 { "A" } else { "B" }
                ));
            } else {
                self.emit("    MOVLW 0x00".to_string());
            }
            let (ra, rf) = self.operand(rem_base + i);
            let mne = if i == 0 { "ADDWF" } else { "ADDWFC" };
            self.emit(format!(
                "    {mne} 0x{rf:03X},F,{}",
                if ra == 0 { "A" } else { "B" }
            ));
        }
        self.emit_label(&l_next);
        let (ca, cf) = self.operand(cnt);
        self.emit(format!(
            "    DECFSZ 0x{cf:03X},F,{}",
            if ca == 0 { "A" } else { "B" }
        ));
        self.emit(format!("    BRA {l_loop}"));
    }

    /// The restoring-division recipe for `den_bytes` = 1, 2, or 4, quotient
    /// or remainder selected by `quotient`. The remainder is 2 bytes for a
    /// u8 divide (the u8 rem shift can carry, PIC14 layout contract), and
    /// `den_bytes` bytes otherwise; the loop counter sits after the rem.
    fn emit_divmod(&mut self, name: &str, den_bytes: u8, scr: u16, quotient: bool) {
        let num = self.slot_addr(name, "num").direct();
        let den = self.slot_addr(name, "den").direct();
        let rem_bytes = den_bytes.max(2);
        self.emit_divmod_loop(
            num,
            den,
            scr,
            scr + u16::from(rem_bytes),
            den_bytes,
            rem_bytes,
        );
        if quotient {
            self.store_retval(num, den_bytes);
        } else {
            self.store_retval(scr, den_bytes);
        }
        self.emit("    RETURN".to_string());
    }

    /// The signed div/mod wrapper: abs both operands in place in the param
    /// slots (unsigned abs, INT_MIN safe), run the unsigned divmod with
    /// `rem` at `__scr[1..]` and the counter after it (byte 0 holds the
    /// flags: bit0 = negate quotient = num<0 XOR den<0, bit1 = negate
    /// remainder = num<0), then negate per the flags.
    fn emit_sdivmod(&mut self, name: &str, den_bytes: u8, scr: u16, quotient: bool) {
        let num = self.slot_addr(name, "num").direct();
        let den = self.slot_addr(name, "den").direct();
        let num_hi = num + u16::from(den_bytes) - 1;
        let den_hi = den + u16::from(den_bytes) - 1;
        let rem_bytes = den_bytes.max(2);
        let l_den = self.fresh_label();
        let l_go = self.fresh_label();
        let l_store = self.fresh_label();
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    CLRF 0x{sf:03X},{}",
            if sa == 0 { "A" } else { "B" }
        )); // flags = 0
        let (na, nf) = self.operand(num_hi);
        self.emit(format!(
            "    BTFSS 0x{nf:03X},7,{}",
            if na == 0 { "A" } else { "B" }
        )); // num < 0?
        self.emit(format!("    BRA {l_den}"));
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    BSF 0x{sf:03X},1,{}",
            if sa == 0 { "A" } else { "B" }
        )); // bit1: remainder sign follows dividend
        self.emit(format!(
            "    BSF 0x{sf:03X},0,{}",
            if sa == 0 { "A" } else { "B" }
        )); // bit0: quotient negate: num<0
        self.neg_in_place(num, den_bytes); // num = |num|
        self.emit_label(&l_den);
        let (da, df) = self.operand(den_hi);
        self.emit(format!(
            "    BTFSS 0x{df:03X},7,{}",
            if da == 0 { "A" } else { "B" }
        )); // den < 0?
        self.emit(format!("    BRA {l_go}"));
        self.neg_in_place(den, den_bytes); // den = |den|
        self.emit("    MOVLW 0x01".to_string());
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    XORWF 0x{sf:03X},F,{}",
            if sa == 0 { "A" } else { "B" }
        )); // bit0 ^= den<0
        self.emit_label(&l_go);
        self.emit_divmod_loop(
            num,
            den,
            scr + 1,
            scr + 1 + u16::from(rem_bytes),
            den_bytes,
            rem_bytes,
        );
        if quotient {
            let (sa, sf) = self.operand(scr);
            self.emit(format!(
                "    BTFSS 0x{sf:03X},0,{}",
                if sa == 0 { "A" } else { "B" }
            ));
            self.emit(format!("    BRA {l_store}"));
            self.neg_in_place(num, den_bytes); // -quotient
        } else {
            let (sa, sf) = self.operand(scr);
            self.emit(format!(
                "    BTFSS 0x{sf:03X},1,{}",
                if sa == 0 { "A" } else { "B" }
            ));
            self.emit(format!("    BRA {l_store}"));
            self.neg_in_place(scr + 1, den_bytes); // -remainder
        }
        self.emit_label(&l_store);
        if quotient {
            self.store_retval(num, den_bytes);
        } else {
            self.store_retval(scr + 1, den_bytes);
        }
        self.emit("    RETURN".to_string());
    }

    /// The variable-count shift recipe (all nine `__shl_*`/`__lshr_*`/
    /// `__ashr_*`): mask the count to `width-1` (`__scr[0]` = masked
    /// count), then a bounded loop over the `val` param slot with
    /// `RLCF`/`RRCF`, `DECFSZ` on the masked count. `asl` sets C from the
    /// sign bit before each shift so the sign fills every vacated bit.
    fn emit_shift_body(&mut self, name: &str, bytes: u8, scr: u16, op: ir::BinOp) {
        let val = self.slot_addr(name, "val").direct();
        let hi = val + u16::from(bytes) - 1;
        let mask: u8 = match bytes {
            1 => 0x07,
            2 => 0x0F,
            4 => 0x1F,
            _ => unreachable!(),
        };
        let (ca, cf) = self.operand(self.slot_addr(name, "cnt").direct());
        self.emit(format!(
            "    MOVF 0x{cf:03X},W,{}",
            if ca == 0 { "A" } else { "B" }
        ));
        self.emit(format!("    ANDLW 0x{mask:02X}")); // count & (width-1)
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    MOVWF 0x{sf:03X},{}",
            if sa == 0 { "A" } else { "B" }
        ));
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    MOVF 0x{sf:03X},F,{}",
            if sa == 0 { "A" } else { "B" }
        )); // Z = (cnt == 0)
        self.emit("    BTFSC 0xFD8,2,A".to_string()); // STATUS Z // skip the GOTO when cnt != 0
        self.emit(format!("    BRA {l_done}"));
        self.emit_label(&l_loop);
        match op {
            ir::BinOp::Shl => {
                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                for i in 0..u16::from(bytes) {
                    let (va, vf) = self.operand(val + i);
                    self.emit(format!(
                        "    RLCF 0x{vf:03X},F,{}",
                        if va == 0 { "A" } else { "B" }
                    ));
                }
            }
            ir::BinOp::LShr => {
                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                for i in (0..u16::from(bytes)).rev() {
                    let (va, vf) = self.operand(val + i);
                    self.emit(format!(
                        "    RRCF 0x{vf:03X},F,{}",
                        if va == 0 { "A" } else { "B" }
                    ));
                }
            }
            ir::BinOp::AShr => {
                let (ha, hf) = self.operand(hi);
                self.emit(format!(
                    "    BTFSC 0x{hf:03X},7,{}",
                    if ha == 0 { "A" } else { "B" }
                ));
                self.emit("    BSF 0xFD8,0,A".to_string()); // STATUS C
                let (ha, hf) = self.operand(hi);
                self.emit(format!(
                    "    BTFSS 0x{hf:03X},7,{}",
                    if ha == 0 { "A" } else { "B" }
                ));
                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                for i in (0..u16::from(bytes)).rev() {
                    let (va, vf) = self.operand(val + i);
                    self.emit(format!(
                        "    RRCF 0x{vf:03X},F,{}",
                        if va == 0 { "A" } else { "B" }
                    ));
                }
            }
            _ => unreachable!(),
        }
        let (sa, sf) = self.operand(scr);
        self.emit(format!(
            "    DECFSZ 0x{sf:03X},F,{}",
            if sa == 0 { "A" } else { "B" }
        ));
        self.emit(format!("    BRA {l_loop}"));
        self.emit_label(&l_done);
        self.store_retval(val, bytes);
        self.emit("    RETURN".to_string());
    }

    /// Swap two bytes via the XOR trick (no scratch needed). Each XORWF
    /// consumes its operand from W, so W must be reloaded between the steps
    /// (a stale W from the first load would zero the first byte instead of
    /// swapping).
    fn emit_xor_swap(&mut self, x: u16, y: u16) {
        self.emit_banked("MOVF", y, ",W");
        self.emit_banked("XORWF", x, ",F");
        self.emit_banked("MOVF", x, ",W");
        self.emit_banked("XORWF", y, ",F");
        self.emit_banked("MOVF", y, ",W");
        self.emit_banked("XORWF", x, ",F");
    }

    /// Emit one banked memory op: `op 0x{low:03X}{rest},{a-bit}` with the
    /// operand() bank selection (MOVLB when the slot's bank differs from
    /// the tracked BSR). `rest` is the middle of the operand (e.g. "W",
    /// "F", ", 7" for a bit op, "" for a plain file op).
    fn emit_banked(&mut self, op: &str, addr: u16, rest: &str) {
        let (a, low) = self.operand(addr);
        let abit = if a == 0 { "A" } else { "B" };
        self.emit(format!("    {op} 0x{low:03X}{rest},{abit}"));
    }

    fn emit_f32_extract(&mut self, slot: u16, sign: u16, exp: u16, mant: u16, flip: bool) {
        self.emit_banked("MOVF", slot + 3, ",W");
        self.emit("    ANDLW 0x80".to_string());
        if flip {
            self.emit("    XORLW 0x80".to_string());
        }
        self.emit_banked("MOVWF", sign, "");
        // exp = (b3 & 0x7F) << 1 | (b2 >> 7)
        self.emit_banked("MOVF", slot + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", exp, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", exp, ",F");
        self.emit_banked("BTFSC", slot + 2, ", 7");
        self.emit_banked("BSF", exp, ", 0");
        // mant = b0, b1, (b2 & 0x7F) | 0x80 (the implicit bit, except for
        // a denormal, exp 0, which has no implicit bit).
        self.emit_banked("MOVF", slot, ",W");
        self.emit_banked("MOVWF", mant, "");
        self.emit_banked("MOVF", slot + 1, ",W");
        self.emit_banked("MOVWF", mant + 1, "");
        self.emit_f32_mant_hi(slot);
        self.emit_banked("MOVWF", mant + 2, "");
        // A denormal (exp 0, fraction nonzero) aligns at the exp-1 scale:
        // its value is frac x 2^-149 = frac x 2^(1-127-23), so the
        // alignment treats it as exp 1 with the raw fraction (no implicit
        // bit). ±0 (exp 0, fraction 0) stays exp 0.
        let _l_den = self.fresh_label();
        let l_den_done = self.fresh_label();
        self.emit_banked("MOVF", exp, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit_banked("MOVF", mant, ",W");
        self.emit_banked("IORWF", mant + 1, ",W");
        self.emit_banked("IORWF", mant + 2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("MOVWF", exp, "");
        self.emit(format!("{l_den_done}:"));
    }

    /// RNE round-up: mantissa += 1 across the 3 bytes; on a 24-bit carry the
    /// mantissa renormalizes to 0x800000 with `e += 1`. The carry can only
    /// fire on a full mantissa (m2 == 0xFF, top bit set, a normal), so at
    /// e == 1 it renormalizes into e == 2 (the smallest-normal binade),
    /// which is correct: 0xFFFFFF x 2^-149 + half-ulp rounds to 2^-125.
    fn emit_f32_round_up(&mut self, m0: u16, m1: u16, m2: u16, e: u16) {
        let l_renorm = self.fresh_label();
        let l_done = self.fresh_label();
        self.emit_banked("INCF", m0, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", m1, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", m2, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_renorm}:"));
        self.emit(format!("    MOVLW 0x80"));
        self.emit_banked("MOVWF", m2, "");
        self.emit_banked("CLRF", m1, "");
        self.emit_banked("CLRF", m0, "");
        self.emit(format!("    MOVLW 0x01"));
        self.emit_banked("ADDWF", e, ",F");
        self.emit(format!("{l_done}:"));
    }

    /// Assemble the result into the fixed retval region (0x71-0x74): b0 =
    /// m0, b1 = m1, b2 = (m2 & 0x7F) | (e & 1) << 7, b3 = (e >> 1) | sign.
    fn emit_f32_assemble(&mut self, sign: u16, e: u16, m0: u16, m1: u16, m2: u16) {
        let r = self.retval_lo;
        self.emit_banked("MOVF", m0, ",W");
        self.emit_banked("MOVWF", r, "");
        self.emit_banked("MOVF", m1, ",W");
        self.emit_banked("MOVWF", r + 1, "");
        self.emit(format!("    MOVLW 0x7F"));
        self.emit_banked("ANDWF", m2, ",W");
        self.emit_banked("MOVWF", r + 2, "");
        self.emit_banked("BTFSC", e, ", 0");
        self.emit_banked("BSF", r + 2, ", 7");
        self.emit_banked("MOVF", e, ",W");
        self.emit_banked("MOVWF", r + 3, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", r + 3, ",F");
        self.emit_banked("BTFSC", sign, ", 7");
        self.emit_banked("BSF", r + 3, ", 7");
        self.emit("    RETURN".to_string());
    }

    /// Load `slot+2`'s fraction into W and OR the implicit bit unless the
    /// operand is a denormal (full 8-bit exponent 0, no implicit bit,
    /// ). The caller stores W into the mantissa's high byte. (epic-cc#11)
    fn emit_f32_mant_hi(&mut self, slot: u16) {
        let l_imp = self.fresh_label();
        let l_done = self.fresh_label();
        // denormal check: exp 0 = (b3 & 0x7F) == 0 && !(b2 bit 7)
        self.emit_banked("MOVF", slot + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_imp}"));
        self.emit_banked("BTFSC", slot + 2, ", 7");
        self.emit(format!("    GOTO {l_imp}"));
        // exp 0 (denormal): fraction only, no implicit bit.
        self.emit_banked("MOVF", slot + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_imp}:"));
        self.emit_banked("MOVF", slot + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("{l_done}:"));
    }

    /// Emit the fixed quiet-NaN result (0x7FC00000 | sign) and RETURN.
    /// The sign is the caller's computed result sign (IEEE leaves the NaN
    /// sign unspecified; the class is what matters).
    fn emit_f32_nan(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit_banked("CLRF", r, "");
        self.emit_banked("CLRF", r + 1, "");
        self.emit("    MOVLW 0xC0".to_string());
        self.emit_banked("MOVWF", r + 2, "");
        self.emit("    MOVLW 0x7F".to_string());
        self.emit_banked("MOVWF", r + 3, "");
        self.emit_banked("BTFSC", sign, ", 7");
        self.emit_banked("BSF", r + 3, ", 7");
        self.emit("    RETURN".to_string());
    }

    /// Emit the fixed infinity result (0x7F800000 | sign) and RETURN.
    fn emit_f32_inf(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit_banked("CLRF", r, "");
        self.emit_banked("CLRF", r + 1, "");
        self.emit("    MOVLW 0x80".to_string());
        self.emit_banked("MOVWF", r + 2, "");
        self.emit("    MOVLW 0x7F".to_string());
        self.emit_banked("MOVWF", r + 3, "");
        self.emit_banked("BTFSC", sign, ", 7");
        self.emit_banked("BSF", r + 3, ", 7");
        self.emit("    RETURN".to_string());
    }

    /// Implements `__add_f32`/`__sub_f32` over the contract scratch layout.
    /// Aligns the smaller mantissa right by the clamped exponent gap, then
    /// adds or subtracts at the larger scale. The shifted-out bits build an
    /// exact 24-bit fraction window with round and sticky, so both paths
    /// round RNE from the guard and sticky: no rounding bit goes missing.
    fn emit_f32_add_body(&mut self, scr: u16) {
        let (sa, ea) = (scr, scr + 1);
        let (ma0, ma1, ma2) = (scr + 2, scr + 3, scr + 4);
        let (sb, eb) = (scr + 5, scr + 6);
        let (mb0, mb1, mb2) = (scr + 7, scr + 8, scr + 9);
        let (stick, cnt) = (scr + 10, scr + 11);
        let (ta0, ta1, ta2) = (scr + 6, scr + 12, scr + 13); // ta0 reuses eb
        let l_ma_nz = self.fresh_label();
        let l_copy_b = self.fresh_label();
        let l_zero = self.fresh_label();
        let l_ma_nz2 = self.fresh_label();
        let l_no_swap = self.fresh_label();
        let l_no_clamp = self.fresh_label();
        let l_align_loop = self.fresh_label();
        let l_align_done = self.fresh_label();
        let l_sub = self.fresh_label();
        let l_add_carry = self.fresh_label();
        let l_cmp_b1 = self.fresh_label();
        let l_cmp_b0 = self.fresh_label();
        let l_cmp_frac = self.fresh_label();
        let l_sub_equal_frac = self.fresh_label();
        let l_sub_swap = self.fresh_label();
        let l_sub_done = self.fresh_label();
        let l_sub_no_frac = self.fresh_label();
        let l_sub_borrow_done = self.fresh_label();
        let l_normalize = self.fresh_label();
        let l_sub_guard = self.fresh_label();
        let l_round_step = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_zs_clear = self.fresh_label();
        let l_zs_done = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_inf = self.fresh_label();
        let l_a_not_inf = self.fresh_label();
        let l_b_not_inf = self.fresh_label();
        let l_a_nan_done = self.fresh_label();
        let l_b_nan_done = self.fresh_label();
        let l_inf_done = self.fresh_label();
        // IEEE specials : NaN and infinity operands.
        // NaN a: exp 0xFF (ea == 0xFF) && FRACTION nonzero (the extracted
        // mantissa carries the implicit bit, so inf's 0x800000 must not
        // read as a NaN, test ma2 & 0x7F | ma1 | ma0). `cnt` is dead at
        // this point (the alignment sets it later). (epic-cc#11)
        self.emit_banked("MOVF", ea, ",W");
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nan_done}"));
        self.emit_banked("MOVF", ma0, ",W");
        self.emit_banked("IORWF", ma1, ",W");
        self.emit_banked("MOVWF", cnt, "");
        self.emit_banked("MOVF", ma2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nan_done}:"));
        // NaN b
        self.emit_banked("MOVF", eb, ",W");
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nan_done}"));
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("IORWF", mb1, ",W");
        self.emit_banked("MOVWF", cnt, "");
        self.emit_banked("MOVF", mb2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_nan_done}:"));
        // inf a? (exp 0xFF, mantissa 0, the NaN checks above already
        // routed mantissa-nonzero exp-0xFF operands to l_nan).
        self.emit_banked("MOVF", ea, ",W");
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_inf}"));
        // a is inf: b inf? both inf -> same sign inf, opposite NaN.
        self.emit_banked("MOVF", eb, ",W");
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_inf}"));
        self.emit_banked("MOVF", sa, ",W");
        self.emit_banked("XORWF", sb, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_not_inf}:"));
        // a inf, b finite: result inf (a's sign).
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_not_inf}:"));
        // a finite: b inf? result inf (b's sign).
        self.emit_banked("MOVF", eb, ",W");
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf_done}"));
        self.emit_banked("MOVF", sb, ",W");
        self.emit_banked("MOVWF", sa, "");
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sa);
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sa);
        self.emit(format!("{l_inf_done}:"));
        // zero operand handling.
        self.emit_banked("MOVF", ma0, ",W");
        self.emit_banked("IORWF", ma1, ",W");
        self.emit_banked("IORWF", ma2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ma_nz}"));
        // ma == 0: mb == 0 -> +/-0, else the result is b exactly.
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("IORWF", mb1, ",W");
        self.emit_banked("IORWF", mb2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_copy_b}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_copy_b}:"));
        for (dst, src) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_banked("MOVF", src, ",W");
            self.emit_banked("MOVWF", dst, "");
        }
        self.emit_banked("CLRF", stick, "");
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz}:"));
        // mb == 0 (ma != 0): the result is a exactly.
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("IORWF", mb1, ",W");
        self.emit_banked("IORWF", mb2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ma_nz2}"));
        self.emit_banked("CLRF", stick, "");
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz2}:"));
        self.emit_banked("CLRF", stick, "");
        self.emit_banked("CLRF", ta1, "");
        self.emit_banked("CLRF", ta2, "");
        // swap so that a is the smaller-exponent operand.
        self.emit_banked("MOVF", eb, ",W");
        self.emit_banked("SUBWF", ea, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string()); // C=1 (ea >= eb) -> swap
        self.emit(format!("    GOTO {l_no_swap}"));
        for (x, y) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit(format!("{l_no_swap}:"));
        // ---- alignment: diff = eb: ea (a is the smaller exponent),
        // clamped to 31, shift ma right. The result exponent is the
        // LARGER one (eb): the sum/difference is at its scale, so the
        // result-exp register becomes eb. ----
        self.emit_banked("MOVF", ea, ",W");
        self.emit_banked("SUBWF", eb, ",W");
        self.emit_banked("MOVWF", cnt, "");
        self.emit_banked("MOVF", eb, ",W");
        self.emit_banked("MOVWF", ea, "");
        self.emit_banked("CLRF", ta0, ""); // eb is dead; ta0 = 0
        self.emit("    MOVLW 0x1F".to_string());
        self.emit_banked("SUBWF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_no_clamp}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit_banked("MOVWF", cnt, "");
        self.emit(format!("{l_no_clamp}:"));
        self.emit_banked("MOVF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("    GOTO {l_align_done}"));
        self.emit(format!("{l_align_loop}:"));
        // ma >>= 1; the shifted-out bit enters the TOP of the 24-bit
        // fraction window ta (the last bit out = the round, at ta2 bit 7);
        // bits pushed out the window's bottom accumulate in stick bit 1.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", ma2, ",F");
        self.emit_banked("RRCF", ma1, ",F");
        self.emit_banked("RRCF", ma0, ",F");
        self.emit_banked("RRCF", ta2, ",F");
        self.emit_banked("RRCF", ta1, ",F");
        self.emit_banked("RRCF", ta0, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", stick, ", 1");
        self.emit_banked("BTFSC", ta2, ", 7");
        self.emit_banked("BSF", stick, ", 0");
        self.emit_banked("BTFSS", ta2, ", 7");
        self.emit_banked("BCF", stick, ", 0");
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("{l_align_done}:"));
        // signs equal? add: subtract.
        self.emit_banked("MOVF", sa, ",W");
        self.emit_banked("XORWF", sb, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub}"));
        // add: ma += mb (3-byte carry chain); a carry renormalizes
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("ADDWF", ma0, ",F");
        self.emit_banked("MOVF", mb1, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", mb1, ",W");
        self.emit_banked("ADDWF", ma1, ",F");
        self.emit_banked("MOVF", mb2, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", mb2, ",W");
        self.emit_banked("ADDWF", ma2, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_add_carry}"));
        self.emit(format!("    GOTO {l_round_step}"));
        self.emit(format!("{l_add_carry}:"));
        self.emit_banked("BTFSC", stick, ", 0");
        self.emit_banked("BSF", stick, ", 1");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", ma2, ",F");
        self.emit_banked("RRCF", ma1, ",F");
        self.emit_banked("RRCF", ma0, ",F");
        self.emit_banked("BCF", stick, ", 0");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", stick, ", 0");
        self.emit_banked("BSF", ma2, ", 7");
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("ADDWF", ea, ",F");
        self.emit(format!("    GOTO {l_round_step}"));
        // subtract: compare ma vs mb (the sign follows the larger)
        self.emit(format!("{l_sub}:"));
        self.emit_banked("MOVF", mb2, ",W");
        self.emit_banked("SUBWF", ma2, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_b1}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b1}:"));
        self.emit_banked("MOVF", mb1, ",W");
        self.emit_banked("SUBWF", ma1, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_b0}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b0}:"));
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("SUBWF", ma0, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_frac}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        // ma == mb: |a| == |b| iff the fraction is 0, else a is larger by
        // exactly the fraction (the value is frac, sign = sa).
        self.emit(format!("{l_cmp_frac}:"));
        self.emit_banked("MOVF", ta0, ",W");
        self.emit_banked("IORWF", ta1, ",W");
        self.emit_banked("IORWF", ta2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_equal_frac}"));
        self.emit_banked("BTFSS", stick, ", 1");
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_sub_equal_frac}:"));
        self.emit_banked("BTFSC", stick, ", 1");
        self.emit_banked("BSF", ta0, ", 0");
        self.emit_banked("CLRF", ma0, "");
        self.emit_banked("CLRF", ma1, "");
        self.emit_banked("CLRF", ma2, "");
        self.emit(format!("    GOTO {l_normalize}"));
        self.emit(format!("{l_sub_swap}:"));
        for (x, y) in [(ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit_banked("MOVF", sb, ",W");
        self.emit_banked("MOVWF", sa, "");
        self.emit(format!("{l_sub_done}:"));
        // ---- fractional borrow: the exact result is (ma: mb) - frac, so
        // for frac != 0 the integer part borrows (ma -= 1) and the
        // fraction becomes 2^24: ta (the deep OR folded into ta's
        // LSB first, it is below the 24-bit window, sticky-typed).
        // frac == 0 skips straight to the plain 3-byte subtract. ----
        self.emit_banked("BTFSC", stick, ", 1");
        self.emit_banked("BSF", ta0, ", 0");
        self.emit_banked("MOVF", ta0, ",W");
        self.emit_banked("IORWF", ta1, ",W");
        self.emit_banked("IORWF", ta2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_no_frac}"));
        self.emit_banked("COMF", ta0, ",F");
        self.emit_banked("COMF", ta1, ",F");
        self.emit_banked("COMF", ta2, ",F");
        self.emit_banked("INCF", ta0, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", ta1, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", ta2, ",F");
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", ma0, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", ma1, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", ma2, ",F");
        self.emit(format!("{l_sub_borrow_done}:"));
        self.emit(format!("{l_sub_no_frac}:"));
        // ma -= mb (3-byte borrow chain)
        self.emit_banked("MOVF", mb0, ",W");
        self.emit_banked("SUBWF", ma0, ",F");
        self.emit_banked("MOVF", mb1, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", mb1, ",W");
        self.emit_banked("SUBWF", ma1, ",F");
        self.emit_banked("MOVF", mb2, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", mb2, ",W");
        self.emit_banked("SUBWF", ma2, ",F");
        self.emit(format!("    GOTO {l_normalize}"));
        // ---- normalize the 6-byte value: while !(ma2 bit 7) && ea > 1:
        // (ta:ma) <<= 1 (the fraction's bits move into the mantissa),
        // ea--. Only the subtract path reaches this (a sum never
        // normalizes). The loop stops at ea == 1, NOT 0: a denormal
        // result is the raw fraction at the exp-1 scale (value =
        // frac x 2^-149), and the denormal conversion below drops it
        // to exp 0. Stopping at 0 would leave ma = 2 x frac, doubling
        // the stored value. ----
        self.emit(format!("{l_normalize}:"));
        self.emit_banked("MOVF", ma2, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit_banked("MOVF", ea, ",W");
        self.emit("    SUBLW 0x01".to_string()); // ea == 1 -> stop
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", ta0, ",F");
        self.emit_banked("RLCF", ta1, ",F");
        self.emit_banked("RLCF", ta2, ",F");
        self.emit_banked("RLCF", ma0, ",F");
        self.emit_banked("RLCF", ma1, ",F");
        self.emit_banked("RLCF", ma2, ",F");
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", ea, ",F");
        self.emit(format!("    GOTO {l_normalize}"));
        // subtract-path guard: the top fraction bit (ta2 bit 7).
        self.emit(format!("{l_sub_guard}:"));
        self.emit_banked("BTFSC", ta2, ", 7");
        self.emit_banked("BSF", stick, ", 0");
        self.emit_banked("BTFSS", ta2, ", 7");
        self.emit_banked("BCF", stick, ", 0");
        // Rounds RNE: rounds up on round plus sticky or mantissa LSB, where
        // sticky folds the window tail and the deep OR bit. A sum carry
        // promotes the old round into sticky, so both paths share the test.
        // A denormal result (exp 1, top mantissa bit clear) converts to exp 0
        // and emits the raw fraction as is; a rounded 0x800000 keeps exp 1.
        // Both paths join here: add directly, subtract after normalizing to ea 1.
        let l_den_conv = self.fresh_label();
        let l_den_done = self.fresh_label();
        self.emit(format!("{l_round_step}:"));
        self.emit_banked("BTFSS", stick, ", 0");
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit_banked("MOVF", ta0, ",W");
        self.emit_banked("IORWF", ta1, ",W");
        self.emit_banked("IORWF", ta2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", stick, ", 1");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", ma0, ", 0");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(ma0, ma1, ma2, ea);
        self.emit(format!("{l_den_conv}:"));
        self.emit_banked("MOVF", ma2, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit_banked("MOVF", ea, ",W");
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit_banked("CLRF", ea, "");
        self.emit(format!("{l_den_done}:"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sa, ea, ma0, ma1, ma2);
        // zero result: sign = sa & sb, exp 0, mantissa 0.
        self.emit(format!("{l_zero}:"));
        self.emit_banked("BTFSS", sa, ", 7");
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit_banked("BTFSS", sb, ", 7");
        self.emit(format!("    GOTO {l_zs_clear}"));
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit(format!("{l_zs_clear}:"));
        self.emit_banked("BCF", sa, ", 7");
        self.emit(format!("{l_zs_done}:"));
        self.emit_banked("CLRF", ea, "");
        self.emit_banked("CLRF", ma0, "");
        self.emit_banked("CLRF", ma1, "");
        self.emit_banked("CLRF", ma2, "");
        self.emit(format!("    GOTO {l_assemble}"));
    }

    /// Implements `__mul_f32` over the contract scratch layout with the AN526
    /// 24-iteration shift-add: each set multiplier bit accumulates the low part
    /// exactly while its carry feeds the product top. The result normalizes,
    /// then rounds RNE from guard and sticky before assembly.
    fn emit_f32_mul_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (sign, e) = (scr, scr + 1);
        let (bk0, bk1, bk2) = (scr + 3, scr + 4, scr + 5);
        let cnt = scr + 6;
        let (m0, m1, m2, m3) = (scr + 7, scr + 8, scr + 9, scr + 10);
        let (low0, low1, low2) = (scr + 11, scr + 12, scr + 13);
        let l_loop = self.fresh_label();
        let l_skip = self.fresh_label();
        let l_renorm = self.fresh_label();
        let l_norm_check = self.fresh_label();
        let l_norm_left = self.fresh_label();
        let l_norm_left_shift = self.fresh_label();
        let l_norm_right = self.fresh_label();
        let l_extract = self.fresh_label();
        let l_den_conv = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_carry_in = self.fresh_label();
        let l_no_carry = self.fresh_label();
        let l_ehi_c1clear = self.fresh_label();
        let l_ehi_done = self.fresh_label();
        let l_a_exp_done = self.fresh_label();
        let l_b_exp_done = self.fresh_label();
        let l_a_nz = self.fresh_label();
        let l_b_nz = self.fresh_label();
        let l_a_not_ff = self.fresh_label();
        let l_a_inf = self.fresh_label();
        let l_b_not_ff = self.fresh_label();
        let l_b_inf = self.fresh_label();
        let l_a_inf_b_finite = self.fresh_label();
        let l_b_inf_a_finite = self.fresh_label();
        let l_a_mant_implicit = self.fresh_label();
        let l_a_mant_done = self.fresh_label();
        let l_b_mant_implicit = self.fresh_label();
        let l_b_mant_done = self.fresh_label();
        let l_zero = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_inf = self.fresh_label();
        // sign = (a3 ^ b3) & 0x80
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit_banked("XORWF", pb + 3, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit_banked("MOVWF", sign, "");
        // e = ea + eb: 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8 + eb8 (9 bits: S_lo +
        // C0); e_lo = S_lo + 0x81 (C1); e_hi = C0: borrow (borrow = !C1).
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", low0, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", low0, ",F");
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit_banked("BSF", low0, ", 0"); // ea8
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", low1, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", low1, ",F");
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit_banked("BSF", low1, ", 0"); // eb8
                                              // A nonzero exp-zero operand aligns at exp 1 with its raw fraction.
        self.emit_banked("MOVF", low0, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 1, ",W");
        self.emit_banked("IORWF", low2, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("MOVWF", low0, "");
        self.emit(format!("{l_a_exp_done}:"));
        self.emit_banked("MOVF", low1, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 1, ",W");
        self.emit_banked("IORWF", low2, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("MOVWF", low1, "");
        self.emit(format!("{l_b_exp_done}:"));
        self.emit_banked("MOVF", low1, ",W");
        self.emit_banked("ADDWF", low0, ",W");
        self.emit_banked("MOVWF", low0, "");
        self.emit_banked("CLRF", m3, "");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", m3, ", 0"); // m3 bit 0 = C0
        self.emit("    MOVLW 0x81".to_string());
        self.emit_banked("ADDWF", low0, ",W");
        self.emit_banked("MOVWF", e, "");
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_ehi_c1clear}"));
        self.emit_banked("BTFSC", m3, ", 0");
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_c1clear}:"));
        self.emit_banked("BTFSC", m3, ", 0");
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit_banked("MOVWF", e + 1, "");
        // NaN and infinity classification uses the raw exponent/fraction.
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit_banked("BTFSS", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit_banked("BTFSS", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit_banked("BTFSS", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_inf}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_inf}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_not_ff}:"));
        // Finite zero operands produce signed zero; check the complete raw
        // fraction so denormals are not mistaken for zero.
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", low2, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", low2, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sign);
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sign);
        self.emit(format!("{l_b_nz}:"));
        // Normal operands receive the implicit bit; denormals retain raw
        // fractions (their exponents were bumped to one above).
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", pa + 2, "");
        self.emit(format!("    GOTO {l_a_mant_done}"));
        self.emit(format!("{l_a_mant_implicit}:"));
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit_banked("MOVWF", pa + 2, "");
        self.emit(format!("{l_a_mant_done}:"));
        // bk = mb copy (the multiplier, shifted to test bits)
        // bk = mb copy (the multiplier, shifted to test bits)
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("MOVWF", bk0, "");
        self.emit_banked("MOVF", pb + 1, ",W");
        self.emit_banked("MOVWF", bk1, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", bk2, "");
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit(format!("    GOTO {l_b_mant_done}"));
        self.emit(format!("{l_b_mant_implicit}:"));
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit_banked("MOVWF", bk2, "");
        self.emit(format!("{l_b_mant_done}:"));
        // la tracks (ma mod 2^i) << (23-i): each step shifts right and injects
        // ma bit i at bit 22, so the addend stays the exact low contribution
        // for the tested multiplier bit. Seeding the full shifted value instead
        // adds a wrong addend.
        self.emit_banked("CLRF", pb, "");
        self.emit_banked("CLRF", pb + 1, "");
        self.emit_banked("CLRF", pb + 2, "");
        for addr in [m0, m1, m2, m3, low0, low1, low2] {
            self.emit_banked("CLRF", addr, "");
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit_banked("MOVWF", cnt, "");
        self.emit(format!("{l_loop}:"));
        // test the multiplier bit (bk <<= 1, C = the bit)
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", bk0, ",F");
        self.emit_banked("RLCF", bk1, ",F");
        self.emit_banked("RLCF", bk2, ",F");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_skip}"));
        // Adds low plus the addend without carry-in on byte 0: C currently holds
        // the tested multiplier bit, not a carry, so a carry-in adds a spurious
        // plus one per set bit. Upper bytes take the previous byte carry.
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("ADDWF", low0, ",F");
        self.emit_banked("MOVF", pb + 1, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", pb + 1, ",W");
        self.emit_banked("ADDWF", low1, ",F");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", pb + 2, ",W");
        self.emit_banked("ADDWF", low2, ",F");
        // Carries BIT 23 of the 24-bit low sum into m, not the byte carry-out:
        // bit 23 set without byte overflow still advances the product, and the
        // path masks bit 23 out of low to keep it mod 2^23.
        self.emit_banked("BTFSC", low2, ", 7");
        self.emit(format!("    GOTO {l_carry_in}"));
        self.emit(format!("    GOTO {l_no_carry}"));
        self.emit(format!("{l_carry_in}:"));
        self.emit_banked("BCF", low2, ", 7");
        self.emit_banked("INCF", m0, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", m1, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", m2, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", m3, ",F");
        self.emit(format!("{l_no_carry}:"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("ADDWF", m0, ",F");
        self.emit_banked("MOVF", pa + 1, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", pa + 1, ",W");
        self.emit_banked("ADDWF", m1, ",F");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", pa + 2, ",W");
        self.emit_banked("ADDWF", m2, ",F");
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit_banked("ADDWF", m3, ",F");
        self.emit(format!("{l_skip}:"));
        // la = (la >> 1) | (ma bit i << 22): pa bit 0 is ma bit i (pa has
        // been shifted right i times), so the new bit enters at la bit 22.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pb + 2, ",F");
        self.emit_banked("RRCF", pb + 1, ",F");
        self.emit_banked("RRCF", pb, ",F");
        self.emit_banked("BTFSC", pa, ", 0");
        self.emit_banked("BSF", pb + 2, ", 6");
        // addend >>= 1 (pa = ma >> (i+1))
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pa + 2, ",F");
        self.emit_banked("RRCF", pa + 1, ",F");
        self.emit_banked("RRCF", pa, ",F");
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_loop}"));
        // Convert the product into a unified 47-bit register. A renormalized
        // product already has the correct scale after m >>= 1; otherwise the
        // leading zero m3 is dropped by shifting P left once.
        self.emit_banked("BTFSC", m3, ", 0");
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        // The m bytes already occupy P bits 46..23; shift only the low
        // 23-bit portion so P bit 22 becomes the unified guard bit.
        self.emit_banked("RLCF", low0, ",F");
        self.emit_banked("RLCF", low1, ",F");
        self.emit_banked("RLCF", low2, ",F");
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_renorm}:"));
        // m >>= 1; the old m bit 0 is the unified register's guard bit.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("BTFSC", m3, ", 0");
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", m2, ",F");
        self.emit_banked("RRCF", m1, ",F");
        self.emit_banked("RRCF", m0, ",F");
        self.emit_banked("BCF", low2, ", 7");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", low2, ", 7");
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("ADDWF", e, ",F");
        self.emit(format!("{l_norm_check}:"));
        // First handle e < 1 (including the negative 16-bit exponents of
        // tiny products), then left-normalize while e > 1.
        self.emit_banked("BTFSC", e + 1, ", 7");
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit_banked("MOVF", e + 1, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit_banked("MOVF", e, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit(format!("{l_norm_left}:"));
        self.emit_banked("BTFSC", m2, ", 7");
        self.emit(format!("    GOTO {l_extract}"));
        self.emit_banked("MOVF", e + 1, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_left_shift}"));
        self.emit_banked("MOVF", e, ",W");
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_extract}"));
        self.emit(format!("{l_norm_left_shift}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", low0, ",F");
        self.emit_banked("RLCF", low1, ",F");
        self.emit_banked("RLCF", low2, ",F");
        self.emit_banked("RLCF", m0, ",F");
        self.emit_banked("RLCF", m1, ",F");
        self.emit_banked("RLCF", m2, ",F");
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e + 1, ",F");
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_norm_right}:"));
        self.emit_banked("BTFSC", low0, ", 0");
        self.emit_banked("BSF", m3, ", 1");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", m2, ",F");
        self.emit_banked("RRCF", m1, ",F");
        self.emit_banked("RRCF", m0, ",F");
        self.emit_banked("RRCF", low2, ",F");
        self.emit_banked("RRCF", low1, ",F");
        self.emit_banked("RRCF", low0, ",F");
        self.emit_banked("INCF", e, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", e + 1, ",F");
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_extract}:"));
        // guard = unified bit 23; sticky = unified bits 0..22 plus any bits
        // shifted out while producing a denormal.
        self.emit_banked("BTFSS", low2, ", 7");
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit_banked("ANDWF", low2, ",W");
        self.emit_banked("IORWF", low1, ",W");
        self.emit_banked("IORWF", low0, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", m3, ", 1");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", m0, ", 0");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(m0, m1, m2, e);
        self.emit(format!("{l_den_conv}:"));
        // exp 1 with a clear mantissa top is the denormal encoding.
        self.emit_banked("BTFSC", m2, ", 7");
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("MOVF", e + 1, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("MOVF", e, ",W");
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("CLRF", e, "");
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, m0, m1, m2);
        // the +/-0 result (zero operand): sign | 0
        self.emit(format!("{l_zero}:"));
        self.emit_banked("CLRF", self.retval_lo, "");
        self.emit_banked("CLRF", self.retval_lo + 1, "");
        self.emit_banked("CLRF", self.retval_lo + 2, "");
        self.emit_banked("CLRF", self.retval_lo + 3, "");
        self.emit_banked("BTFSC", sign, ", 7");
        self.emit_banked("BSF", self.retval_lo + 3, ", 7");
        self.emit("    RETURN".to_string());
    }

    /// One restoring-division compare/subtract/restore step: rem (4 bytes)
    /// -= den (3 bytes, the top byte is implicitly 0) with the borrow
    /// folds; on underflow (rem < den) add den back. The final C is the
    /// quotient bit: the caller's branch lands at `l_restore` when clear and
    /// sets the bit at `qbit` bit 0 otherwise; `l_next` resumes after.
    fn emit_f32_div_step(&mut self, rem: u16, den: u16, qbit: u16, l_restore: &str, l_next: &str) {
        self.emit_banked("MOVF", den, ",W");
        self.emit_banked("SUBWF", rem, ",F");
        self.emit_banked("MOVF", den + 1, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", den + 1, ",W");
        self.emit_banked("SUBWF", rem + 1, ",F");
        self.emit_banked("MOVF", den + 2, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", den + 2, ",W");
        self.emit_banked("SUBWF", rem + 2, ",F");
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit_banked("SUBWF", rem + 3, ",F");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_restore}"));
        self.emit_banked("BSF", qbit, ", 0");
        self.emit(format!("    GOTO {l_next}"));
        self.emit(format!("{l_restore}:"));
        self.emit_banked("MOVF", den, ",W");
        self.emit_banked("ADDWF", rem, ",F");
        self.emit_banked("MOVF", den + 1, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", den + 1, ",W");
        self.emit_banked("ADDWF", rem + 1, ",F");
        self.emit_banked("MOVF", den + 2, ",W");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCFSZ", den + 2, ",W");
        self.emit_banked("ADDWF", rem + 2, ",F");
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit_banked("ADDWF", rem + 3, ",F");
        self.emit(format!("{l_next}:"));
    }

    /// Implements `__div_f32` over the contract scratch layout. Restoring
    /// iterations yield the floor quotient plus remainder, then extend to the
    /// mantissa plus guard with the leftover remainder as sticky. Zero
    /// divisors give infinity and zero numerators give zero; out-of-range
    /// exponents clamp through the byte arithmetic.
    fn emit_f32_div_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (sign, e) = (scr, scr + 1);
        let (rem0, rem1, rem2, rem3) = (scr + 3, scr + 4, scr + 5, scr + 6);
        let (den0, den1, den2) = (scr + 7, scr + 8, scr + 9);
        let cnt = scr + 10;
        let spare = scr + 11;
        let r = self.retval_lo;
        let l_zero = self.fresh_label();
        let l_inf = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_loop = self.fresh_label();
        let l_restore = self.fresh_label();
        let l_next = self.fresh_label();
        let l_ge1 = self.fresh_label();
        let l_qsave = self.fresh_label();
        let l_floop = self.fresh_label();
        let l_frestore = self.fresh_label();
        let l_fnext = self.fresh_label();
        let l_round = self.fresh_label();
        let l_round_test = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_den_conv = self.fresh_label();
        let l_den_shift = self.fresh_label();
        let l_norm_a = self.fresh_label();
        let l_norm_a_done = self.fresh_label();
        let l_norm_b = self.fresh_label();
        let l_norm_b_done = self.fresh_label();
        let l_ehi_b = self.fresh_label();
        let l_ehi_done = self.fresh_label();
        let l_a_not_ff = self.fresh_label();
        let l_a_inf = self.fresh_label();
        let l_b_not_ff = self.fresh_label();
        let l_b_inf = self.fresh_label();
        let l_a_inf_b_finite = self.fresh_label();
        let l_b_inf_a_finite = self.fresh_label();
        let l_a_nz = self.fresh_label();
        let l_a_imp = self.fresh_label();
        let l_a_ready = self.fresh_label();
        let l_b_imp = self.fresh_label();
        let l_b_ready = self.fresh_label();
        let l_b_nz = self.fresh_label();
        let l_exp_a_done = self.fresh_label();
        let l_exp_b_done = self.fresh_label();
        let l_e_sub_done = self.fresh_label();
        // sign = (a3 ^ b3) & 0x80
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit_banked("XORWF", pb + 3, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit_banked("MOVWF", sign, "");
        // e = ea: eb + 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8: eb8 (S_lo + borrow B);
        // e_lo = S_lo + 0x7F (C1); e_hi = C1: B.
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", spare, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", spare, ",F");
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit_banked("BSF", spare, ", 0"); // ea8
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", e, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", e, ",F");
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit_banked("BSF", e, ", 0"); // eb8
        self.emit_banked("MOVF", e, ",W");
        self.emit_banked("SUBWF", spare, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("CLRF", rem3, "");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit_banked("BSF", rem3, ", 0"); // rem3 bit 0 = borrow B
        self.emit("    ADDLW 0x7F".to_string());
        self.emit_banked("MOVWF", e, "");
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_ehi_b}"));
        self.emit_banked("BTFSC", rem3, ", 0");
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_b}:"));
        self.emit_banked("BTFSS", rem3, ", 0");
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit_banked("MOVWF", e + 1, "");
        // IEEE class dispatch, using the raw exponent and complete fraction.
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit_banked("BTFSS", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit_banked("BTFSS", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit_banked("BTFSS", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_not_ff}:"));
        // finite zero checks include the complete fraction
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("MOVWF", spare, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", spare, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sign);
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sign);
        self.emit(format!("{l_zero}:"));
        self.emit_banked("CLRF", r, "");
        self.emit_banked("CLRF", r + 1, "");
        self.emit_banked("CLRF", r + 2, "");
        self.emit_banked("CLRF", r + 3, "");
        self.emit_banked("BTFSC", sign, ", 7");
        self.emit_banked("BSF", r + 3, ", 7");
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_b_nz}:"));
        // Denormals begin at the exp-1 alignment scale (effective e8 = 1).
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit_banked("INCF", e, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", e + 1, ",F");
        self.emit(format!("{l_exp_a_done}:"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e + 1, ",F");
        self.emit(format!("{l_exp_b_done}:"));
        // Build raw/implicit mantissas, then normalize denormals to bit 23.
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", pa + 2, "");
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit(format!("    GOTO {l_a_ready}"));
        self.emit(format!("{l_a_imp}:"));
        self.emit_banked("BSF", pa + 2, ", 7");
        self.emit(format!("{l_a_ready}:"));
        self.emit_banked("CLRF", rem3, "");
        self.emit(format!("{l_norm_a}:"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_norm_a_done}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", pa, ",F");
        self.emit_banked("RLCF", pa + 1, ",F");
        self.emit_banked("RLCF", pa + 2, ",F");
        self.emit_banked("INCF", rem3, ",F");
        self.emit(format!("    GOTO {l_norm_a}"));
        self.emit(format!("{l_norm_a_done}:"));
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", pb + 2, "");
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit(format!("    GOTO {l_b_ready}"));
        self.emit(format!("{l_b_imp}:"));
        self.emit_banked("BSF", pb + 2, ", 7");
        self.emit(format!("{l_b_ready}:"));
        self.emit_banked("CLRF", cnt, "");
        self.emit(format!("{l_norm_b}:"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_norm_b_done}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", pb, ",F");
        self.emit_banked("RLCF", pb + 1, ",F");
        self.emit_banked("RLCF", pb + 2, ",F");
        self.emit_banked("INCF", cnt, ",F");
        self.emit(format!("    GOTO {l_norm_b}"));
        self.emit(format!("{l_norm_b_done}:"));
        self.emit_banked("MOVF", rem3, ",W");
        self.emit_banked("SUBWF", e, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_e_sub_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e + 1, ",F");
        self.emit(format!("{l_e_sub_done}:"));
        self.emit_banked("MOVF", cnt, ",W");
        self.emit_banked("ADDWF", e, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("INCF", e + 1, ",F");
        // denominator copy, now normalized to [2^23, 2^24).
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("MOVWF", den0, "");
        self.emit_banked("MOVF", pb + 1, ",W");
        self.emit_banked("MOVWF", den1, "");
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit_banked("MOVWF", den2, "");
        // ---- 24 restoring iterations: num <<= 1; rem = rem << 1 | C;
        // if rem >= den set the quotient bit else restore ----
        for addr in [rem0, rem1, rem2, rem3] {
            self.emit_banked("CLRF", addr, "");
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit_banked("MOVWF", cnt, "");
        self.emit(format!("{l_loop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", pa, ",F");
        self.emit_banked("RLCF", pa + 1, ",F");
        self.emit_banked("RLCF", pa + 2, ",F");
        self.emit_banked("RLCF", rem0, ",F");
        self.emit_banked("RLCF", rem1, ",F");
        self.emit_banked("RLCF", rem2, ",F");
        self.emit_banked("RLCF", rem3, ",F");
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_restore, &l_next);
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_loop}"));
        // Save floor(ma/mb) (0/1, ma >= mb) before the mantissa
        // accumulator clears pa.
        self.emit_banked("CLRF", spare, "");
        self.emit_banked("MOVF", pa, ",W");
        self.emit("    ANDLW 0x01".to_string());
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_qsave}"));
        self.emit_banked("BSF", spare, ", 0");
        self.emit(format!("{l_qsave}:"));
        // ---- 25 more iterations: the mantissa + guard, with the sticky in
        // the remainder ----
        for addr in [pa, pa + 1, pa + 2, pa + 3] {
            self.emit_banked("CLRF", addr, "");
        }
        self.emit("    MOVLW 0x19".to_string());
        self.emit_banked("MOVWF", cnt, "");
        self.emit(format!("{l_floop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", pa, ",F");
        self.emit_banked("RLCF", pa + 1, ",F");
        self.emit_banked("RLCF", pa + 2, ",F");
        self.emit_banked("RLCF", pa + 3, ",F");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", rem0, ",F");
        self.emit_banked("RLCF", rem1, ",F");
        self.emit_banked("RLCF", rem2, ",F");
        self.emit_banked("RLCF", rem3, ",F");
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_frestore, &l_fnext);
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_floop}"));
        // The mantissa: q = ma/mb in [0.5, 2). The fraction loop's 25 bits
        // are q's bits 2^-1..2^-25 (pa: f1 at bit 24 .. f25 at bit 0), with
        // the remainder as the sticky. For q < 1 (floor(q) == 0) the
        // mantissa = f1..f24 (f1 = 1 at bit 23) with exp-1; for q >= 1 the
        // mantissa = 1.f1..f23 = 0x800000 | (pa >> 2) with the guard f24
        // (pa bit 1) and the sticky f25 (pa bit 0) | rem. e = ea: eb + 127.
        self.emit_banked("BTFSC", spare, ", 0");
        self.emit(format!("    GOTO {l_ge1}"));
        // q < 1: mantissa = pa >> 1; guard = old pa bit 0; sticky = rem;
        // exp -= 1.
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("SUBWF", e, ",F");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("BTFSC", pa + 3, ", 0");
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pa + 2, ",F");
        self.emit_banked("RRCF", pa + 1, ",F");
        self.emit_banked("RRCF", pa, ",F");
        self.emit_banked("CLRF", spare, "");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", spare, ", 0"); // guard
        self.emit_banked("MOVF", rem0, ",W");
        self.emit_banked("IORWF", rem1, ",W");
        self.emit_banked("IORWF", rem2, ",W");
        self.emit_banked("IORWF", rem3, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit_banked("BSF", spare, ", 1"); // sticky
        self.emit(format!("    GOTO {l_round}"));
        // q >= 1: mantissa = 0x800000 | (pa >> 2); guard = old pa bit 1;
        // sticky = old pa bit 0 | rem.
        self.emit(format!("{l_ge1}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("BTFSC", pa + 3, ", 0");
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pa + 2, ",F");
        self.emit_banked("RRCF", pa + 1, ",F");
        self.emit_banked("RRCF", pa, ",F");
        self.emit_banked("CLRF", spare, "");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", spare, ", 1"); // old bit 0 -> sticky
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pa + 2, ",F");
        self.emit_banked("RRCF", pa + 1, ",F");
        self.emit_banked("RRCF", pa, ",F");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", spare, ", 0"); // guard = old bit 1
        self.emit_banked("BSF", pa + 2, ", 7"); // the leading 1
        self.emit_banked("MOVF", rem0, ",W");
        self.emit_banked("IORWF", rem1, ",W");
        self.emit_banked("IORWF", rem2, ",W");
        self.emit_banked("IORWF", rem3, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit_banked("BSF", spare, ", 1"); // sticky |= rem
                                               // RNE: guard (spare bit 0) && (sticky (spare bit 1) || mantissa LSB)
        self.emit(format!("{l_round}:"));
        // Shift a subnormal result right while e < 1, preserving guard and
        // sticky for the final round-to-nearest-even decision.
        self.emit_banked("BTFSC", e + 1, ", 7");
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit_banked("MOVF", e, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_test}"));
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit(format!("{l_den_shift}:"));
        self.emit_banked("BTFSC", spare, ", 0");
        self.emit_banked("BSF", spare, ", 1");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", pa + 2, ",F");
        self.emit_banked("RRCF", pa + 1, ",F");
        self.emit_banked("RRCF", pa, ",F");
        self.emit_banked("BCF", spare, ", 0");
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit_banked("BSF", spare, ", 0");
        self.emit_banked("INCF", e, ",F");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit_banked("INCF", e + 1, ",F");
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("{l_round_test}:"));
        self.emit_banked("BTFSS", spare, ", 0");
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit_banked("BTFSC", spare, ", 1");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", pa, ", 0");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(pa, pa + 1, pa + 2, e);
        self.emit(format!("{l_den_conv}:"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("MOVF", e + 1, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("MOVF", e, ",W");
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("CLRF", e, "");
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, pa, pa + 1, pa + 2);
    }

    /// The __uitofp_f32 body (scratch: cnt@0, e@1-2, guard@3, stick@4,
    /// spare@5-7; `sign_src` = the byte holding the sign for __sitofp_f32,
    /// or None for the unsigned +0). Leading-1 search: shift the value left
    /// until bit 31 set, counting; e = 127 + 31: shifts; the mantissa is
    /// the shifted value's top 24 bits, guard = bit 7 of the low byte,
    /// sticky = its low 7 bits. Round RNE.
    fn emit_f32_uitofp_body(&mut self, val: u16, scr: u16, sign_src: Option<u16>) {
        let (cnt, e) = (scr, scr + 1);
        let (guard, stick) = (scr + 3, scr + 4);
        let sign = scr + 5;
        let r = self.retval_lo;
        let l_zero = self.fresh_label();
        let l_nz = self.fresh_label();
        let l_loop = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        // zero input -> +/-0 (the sign byte is 0 for uitofp)
        self.emit_banked("MOVF", val, ",W");
        self.emit_banked("IORWF", val + 1, ",W");
        self.emit_banked("IORWF", val + 2, ",W");
        self.emit_banked("IORWF", val + 3, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit_banked("CLRF", r, "");
        self.emit_banked("CLRF", r + 1, "");
        self.emit_banked("CLRF", r + 2, "");
        self.emit_banked("CLRF", r + 3, "");
        if sign_src.is_some() {
            self.emit_banked("BTFSC", sign, ", 7");
            self.emit_banked("BSF", r + 3, ", 7");
        }
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_nz}:"));
        if sign_src.is_none() {
            self.emit_banked("CLRF", sign, "");
        }
        self.emit_banked("CLRF", cnt, "");
        self.emit(format!("{l_loop}:"));
        self.emit_banked("BTFSC", val + 3, ", 7");
        self.emit(format!("    GOTO {l_zero}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        for i in 0..4 {
            self.emit_banked("RLCF", val + i, ",F");
        }
        self.emit_banked("INCF", cnt, ",F");
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_zero}:"));
        // e = 158: cnt; the mantissa is val+1..val+3 (bit 23 = val3 bit 7)
        self.emit_banked("MOVF", cnt, ",W");
        self.emit("    SUBLW 0x9E".to_string()); // 158 - cnt
        self.emit_banked("MOVWF", e, "");
        self.emit_banked("CLRF", e + 1, "");
        self.emit_banked("MOVF", val, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit_banked("MOVWF", guard, "");
        self.emit_banked("MOVF", val, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", stick, "");
        // RNE: guard && (sticky || mantissa LSB)
        self.emit_banked("BTFSS", guard, ", 7");
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit_banked("MOVF", stick, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit_banked("BTFSC", val + 1, ", 0");
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(val + 1, val + 2, val + 3, e);
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, val + 1, val + 2, val + 3);
    }

    /// The __fptoui_f32 / __fptosi_f32 body (scratch: e@0, cnt@1, m@2-4,
    /// sign@5, spare@6 = the result's 4th byte). The biased exponent maps to
    /// a mantissa shift: right by 150: e (truncating; e <= 150), left by
    /// e - 150 (e in 151..158, the u32 range), or a deterministic clamp
    /// (e >= 159 for fptoui -> 0xFFFFFFFF; e >= 158 for fptosi -> +/-0x7F800000
    /// style saturation). `signed` negates the result for a negative input
    /// (the sign is ignored for fptoui; truncation toward zero).
    fn emit_f32_fptoi_body(&mut self, val: u16, scr: u16, signed: bool) {
        let (e, cnt) = (scr, scr + 1);
        let (m0, m1, m2) = (scr + 2, scr + 3, scr + 4);
        let sign = scr + 5;
        let m3 = scr + 6;
        let r = self.retval_lo;
        let l_nz = self.fresh_label();
        let l_left = self.fresh_label();
        let l_rdone = self.fresh_label();
        let l_rloop = self.fresh_label();
        let l_lloop = self.fresh_label();
        let l_posclamp = self.fresh_label();
        let l_store2 = self.fresh_label();
        let l_store = self.fresh_label();
        // e = (b3 & 0x7F) << 1 | (b2 >> 7)
        self.emit_banked("MOVF", val + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("MOVWF", e, "");
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", e, ",F");
        self.emit_banked("BTFSC", val + 2, ", 7");
        self.emit_banked("BSF", e, ", 0");
        if signed {
            self.emit_banked("MOVF", val + 3, ",W");
            self.emit("    ANDLW 0x80".to_string());
            self.emit_banked("MOVWF", sign, "");
        }
        // e == 0 -> result 0 (the sign is dropped for zero)
        self.emit_banked("MOVF", e, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit_banked("CLRF", m0, "");
        self.emit_banked("CLRF", m1, "");
        self.emit_banked("CLRF", m2, "");
        self.emit_banked("CLRF", m3, "");
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_nz}:"));
        // m = the 24-bit mantissa with the implicit bit
        self.emit_banked("MOVF", val, ",W");
        self.emit_banked("MOVWF", m0, "");
        self.emit_banked("MOVF", val + 1, ",W");
        self.emit_banked("MOVWF", m1, "");
        self.emit_banked("MOVF", val + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit_banked("MOVWF", m2, "");
        self.emit_banked("CLRF", m3, "");
        // cnt = 150: e
        self.emit_banked("MOVF", e, ",W");
        self.emit("    SUBLW 0x96".to_string()); // 150 - e
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_left}"));
        self.emit_banked("MOVWF", cnt, "");
        // clamp the right count to 31 (the 24-bit mantissa is zero beyond)
        self.emit("    MOVLW 0x1F".to_string());
        self.emit_banked("SUBWF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_rdone}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit_banked("MOVWF", cnt, "");
        self.emit(format!("{l_rdone}:"));
        self.emit_banked("MOVF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_rloop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RRCF", m2, ",F");
        self.emit_banked("RRCF", m1, ",F");
        self.emit_banked("RRCF", m0, ",F");
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_left}:"));
        // cnt = e - 150 (W = 150: e, negate)
        self.emit("    SUBLW 0x00".to_string());
        self.emit_banked("MOVWF", cnt, "");
        // overflow clamp: fptoui cnt > 8 (e >= 159); fptosi cnt >= 8 (e >= 158)
        if signed {
            self.emit("    MOVLW 0x08".to_string());
        } else {
            self.emit("    MOVLW 0x09".to_string());
        }
        self.emit_banked("SUBWF", cnt, ",W");
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_lloop}"));
        if signed {
            self.emit_banked("BTFSS", sign, ", 7");
            self.emit(format!("    GOTO {l_posclamp}"));
            self.emit_banked("CLRF", m0, "");
            self.emit_banked("CLRF", m1, "");
            self.emit_banked("CLRF", m2, "");
            self.emit("    MOVLW 0x80".to_string());
            self.emit_banked("MOVWF", m3, "");
            self.emit(format!("    GOTO {l_store2}"));
            self.emit(format!("{l_posclamp}:"));
            self.emit("    MOVLW 0xFF".to_string());
            self.emit_banked("MOVWF", m0, "");
            self.emit_banked("MOVWF", m1, "");
            self.emit_banked("MOVWF", m2, "");
            self.emit("    MOVLW 0x7F".to_string());
            self.emit_banked("MOVWF", m3, "");
        } else {
            self.emit("    MOVLW 0xFF".to_string());
            self.emit_banked("MOVWF", m0, "");
            self.emit_banked("MOVWF", m1, "");
            self.emit_banked("MOVWF", m2, "");
            self.emit_banked("MOVWF", m3, "");
        }
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_lloop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit_banked("RLCF", m0, ",F");
        self.emit_banked("RLCF", m1, ",F");
        self.emit_banked("RLCF", m2, ",F");
        self.emit_banked("RLCF", m3, ",F");
        self.emit_banked("DECFSZ", cnt, ",F");
        self.emit(format!("    GOTO {l_lloop}"));
        self.emit(format!("{l_store2}:"));
        if signed {
            // negate the 4-byte result for a negative input (truncation is
            // toward zero, the negate of 0 is 0)
            self.emit_banked("BTFSS", sign, ", 7");
            self.emit(format!("    GOTO {l_store}"));
            for addr in [m0, m1, m2, m3] {
                self.emit_banked("COMF", addr, ",F");
            }
            self.emit_banked("INCF", m0, ",F");
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit_banked("INCF", m1, ",F");
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit_banked("INCF", m2, ",F");
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit_banked("INCF", m3, ",F");
            self.emit(format!("{l_store}:"));
        }
        for (i, addr) in [m0, m1, m2, m3].iter().enumerate() {
            self.emit_banked("MOVF", *addr, ",W");
            self.emit_banked("MOVWF", r + i as u16, "");
        }
        self.emit("    RETURN".to_string());
    }

    /// The __cmp_f32 body (scratch: tmp@0-1; the params are compared in
    /// place with the sign bits cleared for the magnitude compare). NaN
    /// (exp 0xFF, mantissa nonzero) -> 3; both zero (full 8-bit exp == 0,
    /// any signs) -> 0; the sign-magnitude ordering with the sign of the
    /// larger deciding lt/gt (negative values reverse the magnitude order).
    fn emit_f32_cmp_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (tmp0, tmp1) = (scr, scr + 1);
        let r = self.retval_lo;
        let l_nan_a_done = self.fresh_label();
        let l_nan_b_done = self.fresh_label();
        let l_ret3 = self.fresh_label();
        let l_az_done = self.fresh_label();
        let l_ret0 = self.fresh_label();
        let l_sign_diff = self.fresh_label();
        let l_ret1 = self.fresh_label();
        let l_ret2 = self.fresh_label();
        let l_mag_lt = self.fresh_label();
        // NaN a: (b3 & 0x7F) == 0x7F && b2 bit 7 && (b2&0x7F | b1 | b0) != 0
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit_banked("BTFSS", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit_banked("MOVF", pa + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", pa + 1, ",W");
        self.emit_banked("IORWF", pa, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_a_done}:"));
        // NaN b
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit_banked("BTFSS", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit_banked("MOVF", pb + 2, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit_banked("IORWF", pb + 1, ",W");
        self.emit_banked("IORWF", pb, ",W");
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_b_done}:"));
        // both zero (full 8-bit exp == 0, any signs) -> equal. The exponent's
        // LSB lives in b2 bit 7, so the (b3 & 0x7F) test alone swallows the
        // smallest NORMALs (8-bit exp 1: 0x00800000..0x00FFFFFF): skip the
        // zero path when b2 bit 7 is set, mirroring the mul/div zero checks.
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit_banked("BTFSC", pa + 2, ", 7");
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit_banked("MOVF", pb + 3, ",W");
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit_banked("BTFSC", pb + 2, ", 7");
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit(format!("{l_az_done}:"));
        // signs differ? a negative, b positive -> a < b (1); else a > b (2).
        // Mask the XOR to bit 7 (the exponent bits must not pollute it).
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit_banked("XORWF", pb + 3, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sign_diff}"));
        // same sign: save a's sign, clear the sign bits, compare magnitudes
        self.emit_banked("MOVF", pa + 3, ",W");
        self.emit("    ANDLW 0x80".to_string());
        self.emit_banked("MOVWF", tmp1, "");
        self.emit_banked("BCF", pa + 3, ", 7");
        self.emit_banked("BCF", pb + 3, ", 7");
        // equality: OR-accumulate the byte XORs into tmp0
        self.emit_banked("MOVF", pa, ",W");
        self.emit_banked("XORWF", pb, ",W");
        self.emit_banked("MOVWF", tmp0, "");
        for i in 1..4 {
            self.emit_banked("MOVF", pa + i, ",W");
            self.emit_banked("XORWF", pb + i, ",W");
            self.emit_banked("IORWF", tmp0, ",W");
            self.emit_banked("MOVWF", tmp0, "");
        }
        // the 4-byte unsigned compare chain: C = (pa >= pb)
        self.emit_banked("MOVF", pb, ",W");
        self.emit_banked("SUBWF", pa, ",W");
        for i in 1..4 {
            self.emit_banked("MOVF", pb + i, ",W");
            self.emit("    BTFSS 0xFD8,0,A".to_string());
            self.emit_banked("INCFSZ", pb + i, ",W");
            self.emit_banked("SUBWF", pa + i, ",W");
        }
        // equal -> 0; pa < pb -> (negative ? 2: 1); pa > pb -> (negative ? 1: 2)
        self.emit_banked("MOVF", tmp0, ",W");
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_mag_lt}"));
        self.emit_banked("BTFSS", tmp1, ", 7");
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_mag_lt}:"));
        self.emit_banked("BTFSS", tmp1, ", 7");
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("{l_sign_diff}:"));
        self.emit_banked("BTFSS", pa + 3, ", 7");
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_ret0}:"));
        self.emit_banked("CLRF", r, "");
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret1}:"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit_banked("MOVWF", r, "");
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret2}:"));
        self.emit("    MOVLW 0x02".to_string());
        self.emit_banked("MOVWF", r, "");
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret3}:"));
        self.emit("    MOVLW 0x03".to_string());
        self.emit_banked("MOVWF", r, "");
        self.emit("    RETURN".to_string());
    }

    /// Emits one runtime routine body for the current function name. Muls use
    /// hardware `MULWF` partials; divmod and shift loops use real branches,
    /// so a `MOVLB` between test and target stays harmless and the frame
    /// carries no bank constraint. `RLCF`/`RRCF` replace `RLF`/`RRF`. Args
    /// arrive in param slots, results leave in the fixed retval slots, and
    /// scratch lives in `__scr`; an `_isr` copy reads its own slots so the
    /// ISR frame never overlaps main.
    fn emit_routine(&mut self) {
        let name = self.cur_func;
        let scr = self.slot_addr(name, "__scr").direct();
        let recipe = name
            .strip_suffix("_isr_high")
            .or_else(|| name.strip_suffix("_isr"))
            .unwrap_or(name);
        if !ir::is_runtime_routine(name) {
            panic!("isel-pic18: @{name} is not a runtime routine");
        }
        self.emit(format!("{name}:"));
        match recipe {
            "__mul_u8" => self.emit_hw_mul(name, 1, scr),
            "__mul_u16" => self.emit_hw_mul(name, 2, scr),
            "__mul_u32" => self.emit_hw_mul(name, 4, scr),
            "__udiv_u8" => self.emit_divmod(name, 1, scr, true),
            "__urem_u8" => self.emit_divmod(name, 1, scr, false),
            "__udiv_u16" => self.emit_divmod(name, 2, scr, true),
            "__urem_u16" => self.emit_divmod(name, 2, scr, false),
            "__udiv_u32" => self.emit_divmod(name, 4, scr, true),
            "__urem_u32" => self.emit_divmod(name, 4, scr, false),
            "__sdiv_i8" => self.emit_sdivmod(name, 1, scr, true),
            "__srem_i8" => self.emit_sdivmod(name, 1, scr, false),
            "__sdiv_i16" => self.emit_sdivmod(name, 2, scr, true),
            "__srem_i16" => self.emit_sdivmod(name, 2, scr, false),
            "__sdiv_i32" => self.emit_sdivmod(name, 4, scr, true),
            "__srem_i32" => self.emit_sdivmod(name, 4, scr, false),
            "__shl_u8" => self.emit_shift_body(name, 1, scr, ir::BinOp::Shl),
            "__shl_u16" => self.emit_shift_body(name, 2, scr, ir::BinOp::Shl),
            "__shl_u32" => self.emit_shift_body(name, 4, scr, ir::BinOp::Shl),
            "__lshr_u8" => self.emit_shift_body(name, 1, scr, ir::BinOp::LShr),
            "__lshr_u16" => self.emit_shift_body(name, 2, scr, ir::BinOp::LShr),
            "__lshr_u32" => self.emit_shift_body(name, 4, scr, ir::BinOp::LShr),
            "__ashr_i8" => self.emit_shift_body(name, 1, scr, ir::BinOp::AShr),
            "__ashr_i16" => self.emit_shift_body(name, 2, scr, ir::BinOp::AShr),
            "__ashr_i32" => self.emit_shift_body(name, 4, scr, ir::BinOp::AShr),
            "__add_f32" | "__sub_f32" => {
                let pa = self.slot_addr(name, "a").direct();
                let pb = self.slot_addr(name, "b").direct();
                self.emit_f32_extract(pa, scr, scr + 1, scr + 2, false);
                self.emit_f32_extract(pb, scr + 5, scr + 6, scr + 7, recipe == "__sub_f32");
                self.emit_f32_add_body(scr);
            }
            "__mul_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.emit_f32_mul_body(a, b, scr);
            }
            "__div_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.emit_f32_div_body(a, b, scr);
            }
            "__uitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                self.emit_f32_uitofp_body(val, scr, None);
            }
            "__sitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                let sign = scr + 5;
                let l_pos = self.fresh_label();
                self.emit_banked("MOVF", val + 3, ",W");
                self.emit("    ANDLW 0x80".to_string());
                self.emit_banked("MOVWF", sign, "");
                self.emit_banked("BTFSS", val + 3, ", 7");
                self.emit(format!("    GOTO {l_pos}"));
                self.neg_in_place(val, 4);
                self.emit(format!("{l_pos}:"));
                self.emit_f32_uitofp_body(val, scr, Some(sign));
            }
            "__fptoui_f32" | "__fptosi_f32" => {
                let val = self.slot_addr(name, "val").direct();
                self.emit_f32_fptoi_body(val, scr, recipe == "__fptosi_f32");
            }
            "__cmp_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.emit_f32_cmp_body(a, b, scr);
            }
            other => panic!("isel-pic18: no recipe for runtime routine {other}"),
        }
    }
}

/// Emit a `BrCond`'s branch lines for the materializing path: the cond byte
/// has already been loaded into `W`, so `BZ`/`BNZ` test it. Each edge's phi
/// copies run on that edge's own path, and PIC18's conditional branches are
/// real label branches (not PIC14's one-instruction skips), so the copies
/// inline directly with no intermediate copy block.
fn emit_cond_branches<'m>(
    g: &mut Gen<'m>,
    lt: &str,
    lf: &str,
    t_copies: &Option<Vec<(String, Ty, Val)>>,
    f_copies: &Option<Vec<(String, Ty, Val)>>,
    b: &ir::Block,
    doms: &HashMap<String, HashSet<String>>,
    bc: &ir::BrCond,
) {
    match (t_copies, f_copies) {
        (None, None) => {
            g.emit(format!("    BZ {lf}"));
            g.emit(format!("    BRA {lt}"));
        }
        (None, Some(cf)) => {
            g.emit(format!("    BNZ {lt}"));
            emit_phi_copies(g, cf, doms[&b.label].contains(&bc.f));
            g.emit(format!("    BRA {lf}"));
        }
        (Some(ct), None) => {
            g.emit(format!("    BZ {lf}"));
            emit_phi_copies(g, ct, doms[&b.label].contains(&bc.t));
            g.emit(format!("    BRA {lt}"));
        }
        (Some(ct), Some(cf)) => {
            let l_fcopies = g.fresh_label();
            g.emit(format!("    BZ {l_fcopies}"));
            emit_phi_copies(g, ct, doms[&b.label].contains(&bc.t));
            g.emit(format!("    BRA {lt}"));
            g.emit_label(&l_fcopies);
            emit_phi_copies(g, cf, doms[&b.label].contains(&bc.f));
            g.emit(format!("    BRA {lf}"));
        }
    }
}

/// Lower the compare a `BrCond` absorbs, with the branch's two edges as the
/// compare's exits. Each edge that carries phi copies gets a trampoline
/// (copies, then a branch to the real target); copy-free edges branch
/// straight. The compare's own lowering never writes its result slot.
fn emit_fused_branch<'m>(
    g: &mut Gen<'m>,
    c: &ir::Icmp,
    lt: &str,
    lf: &str,
    t_copies: Option<&[(String, Ty, Val)]>,
    f_copies: Option<&[(String, Ty, Val)]>,
    t_dom: bool,
    f_dom: bool,
) {
    // The exits the compare branches to, per edge. A copy-carrying edge
    // routes through a trampoline whose label is emitted after the compare.
    let mut tramps: Vec<(String, Vec<(String, Ty, Val)>, bool, String)> = Vec::new();
    let mut exit = |copies: Option<&[(String, Ty, Val)]>, target: &str, dom: bool| -> String {
        match copies {
            None => target.to_string(),
            Some(cs) => {
                let l = g.fresh_label();
                tramps.push((l.clone(), cs.to_vec(), dom, target.to_string()));
                l
            }
        }
    };
    let l_t = exit(t_copies, lt, t_dom);
    let l_f = exit(f_copies, lf, f_dom);
    let n = c.ty.bytes();
    let fuse = Some((l_t, l_f));
    if c.pred == "eq" || c.pred == "ne" {
        g.emit_icmp_eq_ne(c.a.clone(), c.b.clone(), &c.pred, &c.dst, n, fuse);
    } else {
        debug_assert!(matches!(c.pred.as_str(), "ult" | "uge" | "ugt" | "ule"));
        g.emit_icmp_chain(c.a.clone(), c.b.clone(), &c.pred, &c.dst, n, fuse);
    }
    for (l, copies, dom, target) in tramps {
        g.emit_label(&l);
        emit_phi_copies(g, &copies, dom);
        g.emit(format!("    BRA {target}"));
    }
}

/// Computes classic iterative dominator sets to classify phi-copy edges:
/// `pred -> merge` is a back edge exactly when `merge` dominates `pred`,
/// so the merge slots on that edge hold current-iteration values. The
/// routine stays private to `isel`, so this crate duplicates the
/// algorithm. Covers self-loops and separate-latch back edges.
fn block_dominators(f: &Func) -> HashMap<String, HashSet<String>> {
    let entry = &f.blocks[0].label;
    let all: HashSet<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
    // Predecessor lists from the terminators' targets (the terminator is
    // the last inst of every block).
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &f.blocks {
        let targets: Vec<&str> = match b.insts.last() {
            Some(Inst::Br(br)) => vec![br.target.as_str()],
            Some(Inst::BrCond(bc)) => vec![bc.t.as_str(), bc.f.as_str()],
            _ => vec![],
        };
        for t in targets {
            preds.entry(t).or_default().push(b.label.as_str());
        }
    }
    let mut dom: HashMap<String, HashSet<String>> = HashMap::new();
    dom.insert(entry.clone(), HashSet::from([entry.clone()]));
    for b in &f.blocks {
        if b.label != *entry {
            dom.insert(b.label.clone(), all.clone());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            if b.label == *entry {
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

/// Direct and indirect call edges between module functions, for the
/// emission ordering. Recipes and naked bodies have no `Gen` run, so
/// they are neither sources nor targets: they never enter the exit-bank
/// map, and a caller must treat them as unknown exits.
fn call_edges<'a>(funcs: &[&'a Func]) -> HashMap<&'a str, Vec<&'a str>> {
    let known: std::collections::HashSet<&str> = funcs
        .iter()
        .filter(|f| !ir::is_runtime_routine(&f.name) && !f.naked)
        .map(|f| f.name.as_str())
        .collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for f in funcs {
        if !known.contains(f.name.as_str()) {
            continue;
        }
        for b in &f.blocks {
            for inst in &b.insts {
                let Inst::Call(c) = inst else { continue };
                let targets: Vec<&str> = if c.callees.is_empty() {
                    vec![c.func.as_str()]
                } else {
                    c.callees.iter().map(|s| s.as_str()).collect()
                };
                for t in targets {
                    if known.contains(t) {
                        edges.entry(f.name.as_str()).or_default().push(t);
                    }
                }
            }
        }
    }
    edges
}

/// Callees before callers, ties in module order. The call graph is a
/// DAG (recursion is a compile error upstream), so the scan always
/// progresses; the fallback branch is a defensive cycle break that
/// degrades to master's order and an absent map entry.
fn emission_order<'a>(funcs: &[&'a Func], edges: &HashMap<&str, Vec<&str>>) -> Vec<&'a Func> {
    let mut done: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut ordered = Vec::with_capacity(funcs.len());
    loop {
        let mut next = None;
        for f in funcs {
            if done.contains(f.name.as_str()) {
                continue;
            }
            let ready = edges
                .get(f.name.as_str())
                .map(|ts| ts.iter().all(|t| done.contains(t)))
                .unwrap_or(true);
            if ready {
                next = Some(f);
                break;
            }
        }
        match next {
            Some(f) => {
                done.insert(f.name.as_str());
                ordered.push(*f);
            }
            None => {
                for f in funcs {
                    if !done.contains(f.name.as_str()) {
                        done.insert(f.name.as_str());
                        ordered.push(*f);
                    }
                }
                return ordered;
            }
        }
    }
}

/// The bank a function leaves live on return: the join over its
/// RETURN-ending blocks' recorded end states. Unanimous known ends
/// give the bank; any dirty, missing, or disagreeing end gives
/// unknown.
fn exit_bank(ends: &[Option<u8>]) -> Option<u8> {
    let mut known: Option<u8> = None;
    for e in ends {
        match (known, e) {
            (_, None) => return None,
            (None, Some(v)) => known = Some(*v),
            (Some(v), Some(w)) if v == *w => {}
            (Some(_), Some(_)) => return None,
        }
    }
    known
}

/// Emits dependency-ordered phi copies for one edge: no copy clobbers a
/// slot a later copy still reads. Back edges run readers before writers
/// (the merge slots hold current values); forward edges run writers
/// first (the edge defines each slot). A true cycle needs a temp
/// register and panics: silent emission would miscompile.
fn emit_phi_copies<'m>(g: &mut Gen<'m>, copies: &[(String, Ty, Val)], back_edge: bool) {
    let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
        .iter()
        .map(|(dst, ty, val)| {
            let da = g.slot_addr(g.cur_func, dst).direct();
            let src = match val {
                Val::Reg(r) if g.resolved.contains_key(&iselcore::ssa_key(g.cur_func, r)) => None,
                Val::Reg(r) => Some(g.slot_addr(g.cur_func, r).direct()),
                _ => None,
            };
            (da, src, *ty, val.clone())
        })
        .collect();
    let n = pending.len();
    let mut emitted = vec![false; n];
    let mut emitted_count = 0usize;
    while emitted_count < n {
        let mut progress = false;
        for i in 0..n {
            if emitted[i] {
                continue;
            }
            let (da, src, ty, val) = &pending[i];
            let blocked = if back_edge {
                (0..n).any(|j| !emitted[j] && j != i && pending[j].1 == Some(*da))
            } else {
                match src {
                    Some(s) => (0..n).any(|j| !emitted[j] && j != i && pending[j].0 == *s),
                    None => false,
                }
            };
            if !blocked {
                g.emit_move_val_to_slot(val, *ty, *da);
                emitted[i] = true;
                emitted_count += 1;
                progress = true;
            }
        }
        if !progress {
            panic!("isel-pic18: cyclic phi copies not supported");
        }
    }
}

/// The low-priority ISR's context-save area base (`Some` only in priority
/// mode: the module holds both a high- and a low-priority ISR). `None`
/// in compatibility mode, where the single handler uses the device's fixed
/// save block. Must agree with `alloc`'s layout for the same module: a
/// low ISR with `None` (or `Some` without both priorities) panics below.
///
/// The two epic-cc#477 save areas are `None` here. The driver and every
/// end-to-end harness pass `alloc`'s values through `select_with_locs`;
/// this wrapper exists for unit tests of hand-built modules, which do
/// not model a layout. A module that reaches the prologue without an
/// area panics loudly, so a test cannot silently emit an ISR that saves
/// PROD/FSR1 nowhere.
pub fn select(
    device: &Device,
    m: &Module,
    addrs: &HashMap<String, u16>,
    isr_low_save: Option<u16>,
) -> String {
    select_with_locs(device, m, addrs, isr_low_save, None, None).0
}

/// `select` plus a parallel per-line source-location vector, index-aligned
/// with the returned asm text. `None` marks a compiler-generated line (the
/// header, `__start`, const tables, prologue glue). The driver threads this
/// through to build the address-to-line table.
pub fn select_with_locs(
    device: &Device,
    m: &Module,
    addrs: &HashMap<String, u16>,
    isr_low_save: Option<u16>,
    isr_save: Option<u16>,
    isr_hi_save: Option<u16>,
) -> (String, Vec<Option<SrcLoc>>) {
    let (common_lo, _) = device
        .fixed_retval
        .expect("isel-pic18's fixed retval region needs a fixed_retval reservation");
    // The compat ISR's W save, in the 16-byte fixed block: past the 7 SFR
    // saves (common_lo+1..+7) and before the retval-snapshot backup
    // (common_lo+12..+15), the one byte free of both (epic-cc#356; the
    // naive common_lo+4 collides with FSR0H's own snapshot slot).
    const ISR_W_SAVE_OFFSET: u16 = 8;
    let (_, access_bank_hi) = device
        .access_bank
        .expect("isel-pic18's access-bank frame checks need an access_bank reservation");
    let mut out: Vec<String> = Vec::new();
    let mut locs: Vec<Option<SrcLoc>> = Vec::new();
    out.extend(vec![
        "; pic8 -- P2 integer spine (isel-pic18)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "INTCON equ 0xFF2".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ]);
    locs.extend(std::iter::repeat(None).take(8));
    // RAM globals as `equ` names so hand-written inline asm can reference
    // them XC8-style (`movf _m_a+0,w`): the assembler aliases `_x` to `x`
    // and evaluates `sym+N`, so no label emission is needed (epic-cc#610).
    // Sorted for deterministic output; a const that alloc kept in flash has
    // no RAM address and is absent here, so it keeps its table label.
    let mut ram_globals: Vec<(&String, &u16)> = m
        .globals
        .iter()
        // Every global alloc gave a RAM address, const or not: a const that
        // is used as a plain pointer argument is placed in RAM
        // (epic-cc#443), and `map_text` then emits it as a `global` line, so
        // it is RAM for the W cache and for the `equ` names alike.
        .filter_map(|g| addrs.get(&g.name).map(|a| (&g.name, a)))
        .collect();
    ram_globals.sort();
    // Every byte a RAM global occupies, so no W-cache entry can be
    // grounded in storage an ISR may rewrite behind the compiler's back
    // (epic-cc#502).
    let mut global_addrs: HashSet<u16> = HashSet::new();
    for (name, addr) in &ram_globals {
        // Width by name, from the module, not by re-matching the address.
        let size = m
            .globals
            .iter()
            .find(|g| &g.name == *name)
            .map(|g| g.size as u16)
            .unwrap_or(1);
        for i in 0..size {
            global_addrs.insert(**addr + i);
        }
    }
    for (name, addr) in ram_globals {
        // A RAM global sharing a fixed SFR name would silently resolve to
        // the wrong address either way; fail loudly instead (epic-cc#610).
        if matches!(name.as_str(), "STATUS" | "PRODL" | "PRODH" | "INTCON") {
            panic!("isel-pic18: global {name} collides with a fixed SFR name");
        }
        out.push(format!("{name} equ 0x{addr:03X}"));
        locs.push(None);
    }
    // Core SFR names used by hand-written math asm (`movf PRODL,w`):
    // PRODL/PRODH/STATUS sit at architecture-fixed addresses on every
    // PIC18 (DS39632E Table 5-2), so they are literals here, not device
    // data. INTCON above stays as the historical first equ.
    for (name, addr) in [
        ("STATUS", 0xFD8u16),
        ("PRODL", 0xFF3u16),
        ("PRODH", 0xFF4u16),
    ] {
        out.push(format!("{name} equ 0x{addr:03X}"));
        locs.push(None);
    }
    if !m.module_asm.is_empty() {
        out.push("; module asm".to_string());
        locs.push(None);
        for entry in &m.module_asm {
            for line in entry.split('\n') {
                out.push(line.to_string());
                locs.push(None);
            }
        }
    }
    // Shared across every `Gen` below so `fresh_label` never repeats a
    // `tmp{n}:` label across two different functions in the same output.
    let mut tmp = 0u32;
    // Every pointer reg in the module, folded once up front; later tasks'
    // pointer emitters consume it via `Gen::resolved_for`.
    let resolved = resolve_pointers(m);
    // Priority ISRs (epic-cc#346): at most one handler per vector. A lone
    // handler of any priority keeps the P5 compatibility wiring (body at
    // the 0x0008 vector, fixed save block); a high/low pair uses priority
    // wiring (GOTO stubs at both vectors, floating bodies, the low ISR on
    // its own save area). Two bodies cannot share one vector entry, so a
    // body-at-vector layout only exists for the lone-ISR case.
    let hi_isrs: Vec<&Func> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority == 1)
        .collect();
    let lo_isrs: Vec<&Func> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority != 1)
        .collect();
    assert!(
        hi_isrs.len() <= 1 && lo_isrs.len() <= 1,
        "isel-pic18: at most one high- and one low-priority interrupt handler (found {} high, {} low)",
        hi_isrs.len(),
        lo_isrs.len()
    );
    let priority_mode = !hi_isrs.is_empty() && !lo_isrs.is_empty();
    assert!(
        isr_low_save.is_some() == priority_mode,
        "isel-pic18: low save area must be present exactly in priority mode (both priorities present)"
    );
    // The ISR's own area for the PROD/FSR1 bytes epic-cc#477 adds (PIC18
    // only: PIC14/PIC14E have no MULWF/PROD, so alloc leaves their layout
    // byte-identical). The fixed block has no room, so each ISR context
    // uses an area carved from its region.
    let has_isr = m.funcs.iter().any(|f| f.isr);
    let needs_prod_save = device.core == device::Core::Pic18;
    assert!(
        !needs_prod_save || isr_save.is_some() == (has_isr && !priority_mode),
        "isel-pic18: the compat save area must be present exactly for a non-priority ISR module"
    );
    assert!(
        !needs_prod_save || isr_hi_save.is_some() == priority_mode,
        "isel-pic18: the high save area must be present exactly in priority mode"
    );
    // The carved area a *compat or high* ISR uses for its PROD/FSR1/TABLAT/
    // PCLATH/PCLATU bytes. The low ISR needs no closure arm: its seven
    // bytes are the tail of its own 19-byte area (`s+12..s+18`), not a
    // separate carve.
    let prod_save = || -> u16 {
        if priority_mode {
            isr_hi_save.expect("isel-pic18: high ISR without a high save area")
        } else {
            isr_save.expect("isel-pic18: ISR without a save area")
        }
    };
    let mut funcs: Vec<&Func> = m.funcs.iter().collect();
    // High ISR first, then low ISR, then ordinary functions.
    funcs.sort_by_key(|f| (!f.isr, f.irq_priority != 1));
    // The Gen pass walks callees before callers so a later task can
    // carry each callee's exit banks into its callers; the streamed
    // pass below still concatenates in `funcs` (module) order, so the
    // output text keeps its historical layout.
    let edges = call_edges(&funcs);
    let order = emission_order(&funcs, &edges);
    let mut bodies: HashMap<&str, (Vec<String>, Vec<Option<SrcLoc>>)> = HashMap::new();
    let mut exits: HashMap<String, Option<u8>> = HashMap::new();
    // Priority-mode vectors are GOTO stubs: the two bodies cannot both sit
    // at fixed vector addresses (either body overflows the 16-byte vector
    // gap), so the stubs dispatch to the floating bodies below. The lone
    // ISR keeps the historical body-at-vector layout, emitted in the loop.
    if priority_mode {
        let vectors = device
            .interrupt_vectors
            .get(..2)
            .expect("isel-pic18: priority ISRs need two interrupt vectors");
        for (isr, vector) in [
            (hi_isrs[0].name.as_str(), vectors[0]),
            (lo_isrs[0].name.as_str(), vectors[1]),
        ] {
            out.push(format!("    org 0x{vector:04X}"));
            locs.push(None);
            out.push(format!("    goto {isr}"));
            locs.push(None);
        }
        out.push(String::new());
        locs.push(None);
    }
    // Pass A buffers each ordinary function's body, walking emission
    // order; pass B streams the output in module order. Recipes and
    // naked bodies have no `Gen` run and no buffered body.
    for f in &order {
        if ir::is_runtime_routine(&f.name) || f.naked {
            continue; // streamed by pass B, no Gen run, no map entry
        }
        let mut g = Gen {
            m,
            addrs,
            resolved: &resolved,
            retval_lo: common_lo,
            access_bank_hi,
            bsr: None,
            fwd_join: HashMap::new(),
            bsr_dirty: false,
            exit_banks: &exits,
            fsr0_holds: None,
            pending_copies: Vec::new(),
            cur_func: &f.name,
            global_addrs: &global_addrs,
            w_holds: None,
            isr: f.isr,
            tmp: &mut tmp,
            cur_loc: None,
            out: Vec::new(),
            locs: Vec::new(),
        };
        // Index-based label scheme, matching `isel::select` exactly
        // (`crates/isel/src/lib.rs:4085-4094`): the first block in
        // `f.blocks` gets the bare function name (so `CALL`/`GOTO @func`
        // resolve to it, and it's defined exactly once); every other block
        // gets `{func}_L{label}`. Built once per function, keyed by the
        // IR block's own `label` field so every `Br`/`BrCond` target and
        // Phi-copy successor lookup resolves through this map rather than
        // re-deriving a name inline.
        let mut labels: HashMap<String, String> = HashMap::new();
        for (i, b) in f.blocks.iter().enumerate() {
            let lbl = if i == 0 {
                f.name.clone()
            } else {
                format!("{}_L{}", f.name, b.label)
            };
            labels.insert(b.label.clone(), lbl);
        }
        // Keys phi copies by (predecessor, merge) edge rather than predecessor
        // alone. Keying by predecessor runs both successors copies before the
        // branch and clobbers a loop header slot before the exit edge reads the
        // current value. Mirrors the `isel` scheme.
        let mut phi_copies: HashMap<(String, String), Vec<(String, Ty, Val)>> = HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Phi(p) = inst {
                    for (val, pred) in &p.incoming {
                        phi_copies
                            .entry((pred.clone(), b.label.clone()))
                            .or_default()
                            .push((p.dst.clone(), p.ty, val.clone()));
                    }
                }
            }
        }
        let doms = block_dominators(f);
        // Join-bank tracking (epic-cc#534): predecessor labels per block
        // from terminator targets. Every block ends with a terminator
        // (the match below panics otherwise), so no fall-through edges
        // exist. Switch cases additionally list their dispatch block,
        // whose end state they carry when its lowering selected no
        // banks (when it did, `bsr_dirty` already forced unknown).
        let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
        for b in &f.blocks {
            let targets: Vec<&str> = match b.insts.last() {
                Some(Inst::Br(br)) => vec![br.target.as_str()],
                Some(Inst::BrCond(bc)) => vec![bc.t.as_str(), bc.f.as_str()],
                Some(Inst::Switch(sw)) => {
                    let mut v: Vec<&str> = sw.cases.iter().map(|(_, l)| l.as_str()).collect();
                    v.push(sw.default.as_str());
                    v.push(b.label.as_str());
                    v
                }
                _ => vec![],
            };
            for t in targets {
                preds.entry(t).or_default().push(b.label.as_str());
            }
        }
        let mut block_end: HashMap<String, Option<u8>> = HashMap::new();
        let mut ret_ends: Vec<Option<u8>> = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            g.emit_label(&labels[&b.label]);
            // Join agreement: keep the tracked bank only when every
            // predecessor's recorded end state is present and equal.
            // Unrecorded predecessors are back-edges decided later, so
            // loop headers stay unknown, decided once here. The entry
            // block keeps the cleared state: callers leave any bank.
            if bi > 0 {
                if let Some(ps) = preds.get(b.label.as_str()) {
                    let mut agreed: Option<u8> = None;
                    let mut known = false;
                    let mut ok = true;
                    for p in ps {
                        match block_end.get(*p) {
                            Some(Some(v)) if !known || agreed == Some(*v) => {
                                agreed = Some(*v);
                                known = true;
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && known {
                        g.bsr = agreed;
                    }
                }
            }
            if bi == 0 && f.isr {
                // Saves the preempted main state after the vector entry:
                // the in-flight return value (an ISR call would clobber
                // it), STATUS/BSR/FSRn (banked and pointer work clobbers
                // them), and TBLPTR (a torn mid-setup pointer would
                // misread). W saves last via `MOVWF`, which touches
                // nothing. The low ISR saves the same set to its own
                // area (`isr_low_save`), with a dedicated W slot; the
                // common block uses `ISR_W_SAVE_OFFSET` (see above).
                let low_save = priority_mode && f.irq_priority != 1;
                let prod = prod_save();
                let saves: [(u16, u16); 18] = if low_save {
                    let s = isr_low_save.expect("isel-pic18: low ISR without a low save area");
                    [
                        (common_lo, s + 8),
                        (common_lo + 1, s + 9),
                        (common_lo + 2, s + 10),
                        (common_lo + 3, s + 11),
                        (0xFD8, s + 1),  // STATUS
                        (0xFE0, s + 2),  // BSR
                        (0xFE9, s + 3),  // FSR0L
                        (0xFEA, s + 4),  // FSR0H
                        (0xFE1, s + 12), // FSR1L
                        (0xFE2, s + 13), // FSR1H
                        (0xFF3, s + 14), // PRODL
                        (0xFF4, s + 15), // PRODH
                        (0xFF5, s + 16), // TABLAT
                        (0xFFA, s + 17), // PCLATH
                        (0xFFB, s + 18), // PCLATU
                        (0xFF6, s + 5),  // TBLPTRL
                        (0xFF7, s + 6),  // TBLPTRH
                        (0xFF8, s + 7),  // TBLPTRU
                    ]
                } else {
                    // The fixed block's slots must NOT alias the retval
                    // backup (0x00C-0x00F) with the SFR snapshots: an ISR
                    // that calls a value-returning routine writes the
                    // result into retval 0x000-0x003 mid-handler, which
                    // would destroy any snapshot parked there (STATUS was;
                    // epic-cc#357). Layout: TBLPTR 0x005-0x007, FSR0H
                    // 0x004, W 0x008, STATUS/BSR/FSR0L 0x009-0x00B, retval
                    // backup 0x00C-0x00F. FSR1 and PRODL/PRODH joined in
                    // epic-cc#477 and live in the carved `prod` area, not
                    // here: the block has no free byte, and bytes above it
                    // belong to `ram_banks`. The mul routines leave a
                    // product in PROD across instructions, and the copy
                    // loop holds FSR1 across one, so an ISR that
                    // multiplies or memcpys would otherwise corrupt the
                    // preempted context silently.
                    [
                        (0xFF6, common_lo + 5),  // TBLPTRL
                        (0xFF7, common_lo + 6),  // TBLPTRH
                        (0xFF8, common_lo + 7),  // TBLPTRU
                        (0xFEA, common_lo + 4),  // FSR0H
                        (0xFD8, common_lo + 9),  // STATUS
                        (0xFE0, common_lo + 10), // BSR
                        (0xFE9, common_lo + 11), // FSR0L
                        (0xFE1, prod + 0),       // FSR1L
                        (0xFE2, prod + 1),       // FSR1H
                        (0xFF3, prod + 2),       // PRODL
                        (0xFF4, prod + 3),       // PRODH
                        (0xFF5, prod + 4),       // TABLAT
                        (0xFFA, prod + 5),       // PCLATH
                        (0xFFB, prod + 6),       // PCLATU
                        (common_lo, common_lo + 12),
                        (common_lo + 1, common_lo + 13),
                        (common_lo + 2, common_lo + 14),
                        (common_lo + 3, common_lo + 15),
                    ]
                };
                for (src, dst) in saves {
                    g.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
                }
                if low_save {
                    let s = isr_low_save.expect("isel-pic18: low ISR without a low save area");
                    // The save area can sit in banked RAM (above the
                    // access-bank window): select its bank explicitly
                    // (`operand()`'s idiom) and keep `bsr` honest so the
                    // body needs no re-select. MOVFF cannot read W.
                    let bank = (s >> 8) as u8;
                    g.emit(format!("    MOVLB 0x{bank:X}"));
                    g.bsr = Some(bank);
                    g.emit(format!("    MOVWF 0x{s:03X},B")); // W, last
                } else {
                    g.emit(format!(
                        "    MOVWF 0x{:03X},A",
                        common_lo + ISR_W_SAVE_OFFSET
                    )); // W, last
                }
            }
            // A compare this block's terminator absorbs (see
            // `fusable_icmp`): its lowering moves into the `BrCond` arm
            // below, which knows the branch targets. Emitting it here
            // would materialize a byte nobody reads.
            let fused = Gen::fusable_icmp(&g, f, b).map(|c| c.dst.clone());
            let mut terminator: Option<&Inst> = None;
            for inst in &b.insts {
                match inst {
                    Inst::Phi(_) => {} // eliminated; copies emitted at pred ends
                    Inst::Br(_) | Inst::BrCond(_) | Inst::Switch(_) | Inst::Ret(..) => {
                        terminator = Some(inst)
                    }
                    Inst::Icmp(c) if fused.as_deref() == Some(c.dst.as_str()) => {}
                    other => g.emit_inst(other),
                }
            }
            // Only bank selects inside the terminator lowering below can
            // leave exits holding different banks; prefix selects are
            // identical on every path out of this block.
            g.bsr_dirty = false;
            match terminator {
                Some(Inst::Br(br)) => {
                    g.cur_loc = br.loc.clone();
                    let merge = br.target.clone();
                    if let Some(c) = phi_copies.get(&(b.label.clone(), merge.clone())) {
                        emit_phi_copies(&mut g, c, doms[&b.label].contains(&merge));
                    }
                    g.emit(format!("    BRA {}", labels[&merge]));
                }
                Some(Inst::BrCond(bc)) => {
                    g.cur_loc = bc.loc.clone();
                    let lt = labels[&bc.t].clone();
                    let lf = labels[&bc.f].clone();
                    let t_copies = phi_copies.get(&(b.label.clone(), bc.t.clone())).cloned();
                    let f_copies = phi_copies.get(&(b.label.clone(), bc.f.clone())).cloned();
                    // The absorbed compare, when this branch is its only
                    // consumer: lower it here so its exits are the branch
                    // edges, skipping the 0/1 slot entirely.
                    let fused_icmp = fused.as_deref().and_then(|dst| {
                        b.insts.iter().find_map(|i| match i {
                            Inst::Icmp(ic) if ic.dst == dst => Some(ic),
                            _ => None,
                        })
                    });
                    match fused_icmp {
                        Some(c) => emit_fused_branch(
                            &mut g,
                            c,
                            &lt,
                            &lf,
                            t_copies.as_deref(),
                            f_copies.as_deref(),
                            doms[&b.label].contains(&bc.t),
                            doms[&b.label].contains(&bc.f),
                        ),
                        None => {
                            // Same hazard class as `Select`'s cond (the select guard, see
                            // `emit_inst`'s `Inst::Select` arm): `emit_load_w`'s
                            // `Val::Const` arm emits only `MOVLW`, which does not
                            // set the Z flag (`crates/sim/src/lib.rs`), so a
                            // literal cond here would make `BZ` test a stale flag
                            // from whatever came before instead of `cond`'s real
                            // value. Fail instead of silently miscompiling.
                            assert!(
                                !matches!(bc.cond, Val::Const(_)),
                                "isel-pic18: const cond BrCond not yet supported"
                            );
                            g.emit_load_w(&bc.cond, 0, true);
                            emit_cond_branches(
                                &mut g, &lt, &lf, &t_copies, &f_copies, b, &doms, bc,
                            );
                        }
                    }
                }
                Some(Inst::Switch(sw)) => {
                    g.cur_loc = sw.loc.clone();
                    assert!(
                        !matches!(sw.val, Val::Const(_)),
                        "isel-pic18: constant switch value reached the backend (fold it before isel)"
                    );
                    let l_default = labels[&sw.default].clone();
                    let edge_copies = |l: &str| phi_copies.get(&(b.label.clone(), l.to_string()));
                    let n = sw.cases.len();
                    let base = sw.cases.first().map(|(k, _)| *k).unwrap_or(0);
                    // Table shape: dense-contiguous values base..base+n-1
                    // (irparse sorts them), enough cases for the dispatch
                    // plus table to beat the chain.

                    // Phi-carrying edges route through a per-edge
                    // trampoline (copies, then a branch to the target);
                    // copy-free edges jump straight to their target.

                    // The cost gate compares the table (dispatch words
                    // plus 2 per entry, padding included) against roughly
                    // 6 words per chain link. Negative bases would index
                    // the table backwards while the entry math counts
                    // forward from zero: reject them (the chain handles
                    // negatives fine).
                    let dense = sw.ty.bytes() <= 2
                        && base >= 0
                        && base <= 0xFF
                        && n >= 6
                        && 2 * base + 16 < 4 * n as i64
                        && 4 * (base + n as i64) <= 200
                        && sw
                            .cases
                            .iter()
                            .enumerate()
                            .all(|(i, (k, _))| *k == base + i as i64);
                    // Trampolines for phi-carrying table edges: the table
                    // points at the trampoline, which runs that edge's
                    // copies and branches to the real target.
                    let mut tramps: Vec<(String, Vec<(String, Ty, Val)>, String, String)> =
                        Vec::new();
                    if dense {
                        let l_tbl = g.fresh_label();
                        // Alignment anchor for the assembler's page
                        // fixup: if dispatch plus table would straddle a
                        // 256-byte page, the assembler pads NOPs here to
                        // push the block onto the next page.
                        g.emit("    .pclalign".to_string());
                        // Bounds check, then `ADDWF PCL,F` into a table of
                        // absolute 2-word GOTO entries (4 bytes each, so the
                        // selector needs W = 4*idx). PCL reads as the address
                        // of the next instruction, already one word past the
                        // ADDWF where the table starts: a stray +2 lands
                        // every case on its GOTO's second word and falls
                        // into the next case (epic-cc#484). PCLATH names the
                        // page; `.pcltbl` asserts one-page adjacency.

                        // Default-edge trampoline when the default edge
                        // carries phi copies; the bounds branches target
                        // it instead of the bare default label.
                        let l_def = if let Some(c) = edge_copies(&sw.default) {
                            let t = g.fresh_label();
                            tramps.push((
                                t.clone(),
                                c.clone(),
                                l_default.clone(),
                                sw.default.clone(),
                            ));
                            t
                        } else {
                            l_default.clone()
                        };
                        if sw.ty.bytes() == 2 {
                            g.emit_load_w(&sw.val, 1, true);
                            g.emit(format!("    BNZ {l_def}"));
                        }
                        let slot = g.val_addr(&sw.val).direct();
                        let (a, f) = g.operand(slot);
                        let bank = if a == 0 { "A" } else { "B" };
                        if base == 0 {
                            g.emit_load_w(&sw.val, 0, false);
                            g.emit(format!("    SUBLW 0x{:02X}", (n - 1) as u8));
                            g.emit(format!("    BNC {l_def}"));
                        } else {
                            // Reject idx < base (W = d = idx - base), then
                            // finish the same upper bound on d.
                            g.emit(format!("    MOVLW 0x{base:02X}"));
                            g.emit(format!("    SUBWF 0x{f:03X},W,{bank}"));
                            g.emit(format!("    BNC {l_def}"));
                            g.emit(format!("    SUBLW 0x{:02X}", (n - 1) as u8));
                            g.emit(format!("    BNC {l_def}"));
                        }
                        // Bounds went through W (SUBLW overwrites it), so
                        // the page set comes next and the offset math runs
                        // last, with W live straight into the PCL write.
                        // `slot`/`bank` above still hold: nothing between
                        // touched BSR.
                        g.emit(format!("    MOVLW HIGH({l_tbl})"));
                        g.emit("    MOVWF 0xFFA,A".to_string()); // PCLATH
                                                                 // W = 4*idx: PCL reads as the
                                                                 // address of the next
                                                                 // instruction, which is where
                                                                 // the table starts, so the
                                                                 // index needs no gap word.
                                                                 // Three accumulations of the
                                                                 // index, no scratch byte, and
                                                                 // no flag consumer follows.
                        g.emit_load_w(&sw.val, 0, false);
                        g.emit(format!("    ADDWF 0x{f:03X},W,{bank}"));
                        g.emit(format!("    ADDWF 0x{f:03X},W,{bank}"));
                        g.emit(format!("    ADDWF 0x{f:03X},W,{bank}"));
                        g.emit("    ADDWF 0xFF9,F,A".to_string()); // PCL: the jump
                        g.emit_label(&l_tbl);
                        let span = (base + n as i64) * 4;
                        g.emit(format!("    .pcltbl {l_tbl} {span}"));
                        // Entries for values 0..base cover the gap to a
                        // nonzero base (the bounds check rejects them):
                        // they target the default edge, so a stray index
                        // still lands on defined behavior.
                        let total = (base + n as i64) as usize;
                        let mut tramp_targets: Vec<String> = Vec::with_capacity(total);
                        for _ in 0..base {
                            tramp_targets.push(l_def.clone());
                        }
                        for (_, l) in &sw.cases {
                            if let Some(c) = edge_copies(l) {
                                let t = g.fresh_label();
                                tramps.push((t.clone(), c.clone(), labels[l].clone(), l.clone()));
                                tramp_targets.push(t);
                            } else {
                                tramp_targets.push(labels[l].clone());
                            }
                        }
                        for t in &tramp_targets {
                            g.emit(format!("    GOTO {t}"));
                        }
                        for (t, c, target, orig) in tramps {
                            g.emit_label(&t);
                            emit_phi_copies(&mut g, &c, doms[&b.label].contains(&orig));
                            g.emit(format!("    BRA {target}"));
                        }
                    } else {
                        // Linear equality chain, irparse's expansion
                        // shape emitted in place: each case tests every
                        // byte of the value against the case constant,
                        // mismatches skip to the next case, the hit
                        // edge's phi copies run before its branch.
                        let mask: i64 = match sw.ty.bytes() {
                            1 => 0xFF,
                            2 => 0xFFFF,
                            other => {
                                panic!("isel-pic18: {other}-byte switch not supported (max 2)")
                            }
                        };
                        for (k, l) in &sw.cases {
                            let k = *k & mask;
                            let l_next = g.fresh_label();
                            for b_i in 0..sw.ty.bytes() {
                                let kb = ((k >> (b_i as u32 * 8)) & 0xFF) as u8;
                                g.emit_load_w(&sw.val, b_i, true);
                                g.emit(format!("    SUBLW 0x{kb:02X}"));
                                if b_i as u8 + 1 != sw.ty.bytes() {
                                    g.emit(format!("    BNZ {l_next}"));
                                }
                            }
                            g.emit(format!("    BNZ {l_next}"));
                            if let Some(c) = edge_copies(l) {
                                emit_phi_copies(&mut g, c, doms[&b.label].contains(l));
                            }
                            g.emit(format!("    BRA {}", labels[l]));
                            g.emit_label(&l_next);
                        }
                        if let Some(c) = edge_copies(&sw.default) {
                            emit_phi_copies(&mut g, c, doms[&b.label].contains(&sw.default));
                        }
                        g.emit(format!("    BRA {l_default}"));
                    }
                }
                Some(Inst::Ret(None, loc)) if g.isr => {
                    g.cur_loc = loc.clone();
                    // The ISR restore epilogue replaces `ret`. MOVFF-based
                    // (never touches flags); W restores before STATUS
                    // because MOVF sets Z/N from the moved value. STATUS
                    // last keeps every ISR return flag-transparent: a
                    // Timer2 IRQ inside a main-line XORLW/BNZ dispatch
                    // window mis-dispatched while W came last (epic-cc#604).
                    // Both areas are non-aliased (the fixed block's slots
                    // moved out of retval's 0x000-0x003, epic-cc#357).
                    let low_save = priority_mode && f.irq_priority != 1;
                    let prod = prod_save();
                    if low_save {
                        let s = isr_low_save.expect("isel-pic18: low ISR without a low save area");
                        // Disjoint save area (s+1..+11), so the two halves
                        // below don't alias and their order is free.
                        g.emit_movff_pairs([
                            (s + 11, common_lo + 3),
                            (s + 10, common_lo + 2),
                            (s + 9, common_lo + 1),
                            (s + 8, common_lo),
                        ]);
                        g.emit_movff_pairs([
                            (s + 7, 0xFF8), // TBLPTRU
                            (s + 6, 0xFF7), // TBLPTRH
                            (s + 5, 0xFF6), // TBLPTRL
                            (s + 4, 0xFEA), // FSR0H
                            (s + 3, 0xFE9), // FSR0L
                            (s + 2, 0xFE0), // BSR
                        ]);
                        // The same seven additions as the compat block
                        // (epic-cc#477, epic-cc#532, epic-cc#641): the low
                        // save area grew for them.
                        g.emit_movff_pairs([
                            (s + 12, 0xFE1), // FSR1L
                            (s + 13, 0xFE2), // FSR1H
                            (s + 14, 0xFF3), // PRODL
                            (s + 15, 0xFF4), // PRODH
                            (s + 16, 0xFF5), // TABLAT
                            (s + 17, 0xFFA), // PCLATH
                            (s + 18, 0xFFB), // PCLATU
                        ]);
                    } else {
                        // Dealiased slots (epic-cc#357): STATUS/BSR/FSR0L
                        // live at +9/+10/+11, clear of the retval backup
                        // the group below writes - an ISR-side retval
                        // store can no longer destroy them mid-handler.
                        g.emit_movff_pairs([
                            (common_lo + 7, 0xFF8),  // TBLPTRU
                            (common_lo + 6, 0xFF7),  // TBLPTRH
                            (common_lo + 5, 0xFF6),  // TBLPTRL
                            (common_lo + 4, 0xFEA),  // FSR0H
                            (common_lo + 11, 0xFE9), // FSR0L
                            (common_lo + 10, 0xFE0), // BSR
                        ]);
                        g.emit_movff_pairs([
                            (common_lo + 15, common_lo + 3),
                            (common_lo + 14, common_lo + 2),
                            (common_lo + 13, common_lo + 1),
                            (common_lo + 12, common_lo),
                        ]);
                        // FSR1, PROD and PCLATH/PCLATU, restored before the
                        // retval backup and W (both groups below touch the
                        // retval region through the caller's live result,
                        // not these slots). Order against the SFR group
                        // above is free: the ranges are disjoint.
                        // (epic-cc#477, epic-cc#641)
                        g.emit_movff_pairs([
                            (prod + 0, 0xFE1), // FSR1L
                            (prod + 1, 0xFE2), // FSR1H
                            (prod + 2, 0xFF3), // PRODL
                            (prod + 3, 0xFF4), // PRODH
                            (prod + 4, 0xFF5), // TABLAT
                            (prod + 5, 0xFFA), // PCLATH
                            (prod + 6, 0xFFB), // PCLATU
                        ]);
                    }
                    if low_save {
                        let s = isr_low_save.expect("isel-pic18: low ISR without a low save area");
                        // Banked save area: same explicit select as the
                        // prologue (the body's last bank is tracked in
                        // `bsr`, so this re-select is required, not
                        // redundant).
                        let bank = (s >> 8) as u8;
                        g.emit(format!("    MOVLB 0x{bank:X}"));
                        g.bsr = Some(bank);
                        g.emit(format!("    MOVF 0x{s:03X}, W, B"));
                        // MOVF sets Z/N from W, so STATUS restores after it
                        // (MOVFF leaves flags alone; epic-cc#604).
                        g.emit(format!("    MOVFF 0x{:03X}, 0xFD8", s + 1));
                        // The select above leaves hardware BSR on the save
                        // area's bank, not the preempted value: restore it
                        // through BSR-independent MOVFF before returning,
                        // or main resumes against the wrong bank (epic-cc#534
                        // makes tracked agreement load-bearing there).
                        g.emit(format!("    MOVFF 0x{:03X}, 0xFE0", s + 2));
                        g.bsr = None;
                    } else {
                        g.emit(format!(
                            "    MOVF 0x{:03X}, W, A",
                            common_lo + ISR_W_SAVE_OFFSET
                        ));
                        // MOVF sets Z/N from W, so STATUS restores after it
                        // (MOVFF leaves flags alone; epic-cc#604).
                        g.emit(format!("    MOVFF 0x{:03X}, 0xFD8", common_lo + 9));
                    }
                    g.emit("    RETFIE".to_string());
                }
                Some(Inst::Ret(Some(_), _)) if g.isr => {
                    panic!(
                        "isel-pic18: interrupt handler @{} must be void (cannot return a value)",
                        f.name
                    )
                }
                Some(Inst::Ret(None, loc)) => {
                    g.cur_loc = loc.clone();
                    g.emit("    RETURN".to_string())
                }
                Some(Inst::Ret(Some((ty, v)), loc)) => {
                    g.cur_loc = loc.clone();
                    for i in 0..ty.bytes() {
                        g.emit_load_w(v, i, false);
                        let (a, f2) = g.operand(g.retval_lo + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        g.emit(format!("    MOVWF 0x{f2:03X},{bank}"));
                    }
                    g.emit("    RETURN".to_string());
                }
                _ => panic!("isel-pic18: block has no terminator"),
            }
            // Record for join agreement at successor labels: the end
            // state, unless the terminator lowering selected banks (then
            // exits may disagree and the block contributes unknown).
            // Internal labels inside this block's lowering already reset
            // the tracked state where they join, so only the end counts.
            let end = if g.bsr_dirty { None } else { g.bsr };
            block_end.insert(b.label.clone(), end);
            if matches!(b.insts.last(), Some(Inst::Ret(..))) {
                ret_ends.push(end);
            }
        }
        g.flush_copies();
        // After `bodies` takes `g.out`/`g.locs`: `g` holds `&exits`, so the
        // map may only be mutated once the body has moved out of `g`.
        bodies.insert(f.name.as_str(), (g.out, g.locs));
        // An ISR's epilogue restores the interrupted context's BSR in
        // hardware (`MOVFF ..., BSR`) without the tracked model seeing it,
        // so its recorded end can lie; ISR callees must read as unknown.
        exits.insert(
            f.name.to_string(),
            if f.isr { None } else { exit_bank(&ret_ends) },
        );
    }
    // Pass B streams in module order: the recipe and naked arms keep
    // their verbatim bodies, the ISR vector line keeps its position,
    // and each remaining function pulls its buffered body.
    for f in funcs {
        // the runtime routines: a runtime routine (or its `_isr` copy) emits its recipe body
        // directly: its entry block holds only the `__scr` alloca, which
        // the generic block emitter would render as an empty label
        // (silently falling through into the next function). Every other
        // function takes the ordinary path.
        if ir::is_runtime_routine(&f.name) {
            let mut g = Gen {
                m,
                addrs,
                resolved: &resolved,
                retval_lo: common_lo,
                access_bank_hi,
                bsr: None,
                fwd_join: HashMap::new(),
                bsr_dirty: false,
                exit_banks: &exits,
                fsr0_holds: None,
                pending_copies: Vec::new(),
                cur_func: &f.name,
                global_addrs: &global_addrs,
                w_holds: None,
                isr: f.isr,
                tmp: &mut tmp,
                cur_loc: None,
                out: Vec::new(),
                locs: Vec::new(),
            };
            g.emit_routine();
            g.flush_copies();
            out.extend(g.out);
            locs.extend(g.locs);
            continue;
        }
        // Naked: verbatim, no prologue, panic on non-Asm, barrier markers.
        if f.naked {
            out.push(format!("{}:", f.name));
            locs.push(None);
            out.push("; --- asm start ---".to_string());
            locs.push(None);
            for b in &f.blocks {
                for inst in &b.insts {
                    match inst {
                        Inst::Asm(a) => {
                            // Substitute $0/%0 for rung 4 memory operands
                            let mut substituted = a.template.clone();
                            if !a.operands.is_empty() {
                                for op in &a.operands {
                                    if let Some(reg) = op.ptr.strip_prefix('%') {
                                        if let Some((_, k, terms)) = resolved.get(&ssa_key(&f.name, reg)) {
                                            if *k != 0 || !terms.is_empty() {
                                                panic!("asm: GEP-derived pointers are not supported; operand {} is derived via getelementptr (only direct locals and globals are allowed)", op.ptr);
                                            }
                                        }
                                    }
                                }
                                let mut res = String::with_capacity(substituted.len() + a.operands.len() * 6);
                                let mut chars = substituted.chars().peekable();
                                while let Some(c) = chars.next() {
                                    if c == '$' || c == '%' {
                                        if let Some(&n) = chars.peek() {
                                            if n == '%' || n == '$' { chars.next(); res.push(n); continue; }
                                            if n.is_ascii_digit() {
                                                let mut idx_str = String::new();
                                                while let Some(&d) = chars.peek() { if d.is_ascii_digit() { idx_str.push(d); chars.next(); } else { break; } }
                                                let idx: usize = idx_str.parse().unwrap();
                                                if idx >= a.operands.len() { panic!("asm: placeholder ${idx} out of range for {} operands in template {:?}", a.operands.len(), a.template); }
                                                let ptr = &a.operands[idx].ptr;
                                                let addr = if let Some(g) = ptr.strip_prefix('@') { *addrs.get(g).unwrap_or_else(|| panic!("isel-pic18: no address for @{g}")) } else if let Some(r) = ptr.strip_prefix('%') { *addrs.get(&ssa_key(&f.name, r)).unwrap_or_else(|| panic!("isel-pic18: no slot for {}::{}", f.name, r)) } else { panic!("asm: malformed operand ptr {ptr:?}") };
                                                res.push_str(&format!("0x{addr:02X}"));
                                                continue;
                                            }
                                        }
                                        res.push(c);
                                    } else { res.push(c); }
                                }
                                substituted = res;
                            }
                            for line in substituted.split('\n') {
                                out.push(line.to_string());
                                locs.push(None);
                            }
                        }
                        _ => panic!(
                            "isel-pic18: naked function '{}' contains non-asm instruction; naked bodies must be pure assembly",
                            f.name
                        ),
                    }
                }
            }
            out.push("; --- asm end ---".to_string());
            locs.push(None);
            out.push("".to_string());
            locs.push(None);
            continue;
        }
        if f.isr && !priority_mode {
            // The vector entry at 0x0008 IS the ISR body (the hardware
            // jumps there with GIE cleared; no GOTO indirection, matching
            // PIC14's vector-as-entry convention). `__start`'s reset GOTO
            // at 0x0000 reaches it regardless: PIC18 GOTO/CALL are absolute
            // 20-bit. Priority mode uses GOTO stubs instead (emitted
            // above), so the bodies float.
            out.push("    org 0x0008".to_string());
            locs.push(None);
        }
        let (lines, ls) = bodies
            .get(f.name.as_str())
            .expect("every non-recipe, non-naked function has a buffered body");
        out.extend(lines.clone());
        locs.extend(ls.iter().cloned());
    }
    // `__start` calls `main` and halts; matches the shape `isel::select`
    // uses for its own program entry, minus the ISR machinery (the single-vector mode).
    // Const string literals copied to RAM need init before main.
    // Zero-initialized RAM globals are cleared by one LFSR-seeded CLRF
    // loop per contiguous run (the #486 loop shape with the count in
    // WREG, so no scratch byte and no MOVLB traffic). Runs join adjacent
    // zero-init globals and split at 255 for the one-byte MOVLW count;
    // every run loops, even short ones, to keep a single emission path.
    // (epic-cc#561)
    {
        let mut sorted: Vec<(u16, usize)> = m
            .globals
            .iter()
            .filter(|g| {
                addrs.contains_key(&g.name) && !g.is_const && !g.needs_ram_init() && g.size > 0
            })
            .map(|g| (addrs[&g.name], g.size as usize))
            .collect();
        sorted.sort();
        let mut runs: Vec<(u32, usize)> = Vec::new();
        for (base, len) in sorted {
            match runs.last_mut() {
                Some((rb, rn)) if *rb + *rn as u32 == base as u32 => *rn += len,
                _ => runs.push((base as u32, len)),
            }
        }
        let mut init_zero: Vec<String> = Vec::new();
        for (base, len) in runs {
            let (mut b, mut n) = (base, len);
            while n > 0 {
                let take = n.min(255);
                let l = format!("tmp{tmp}");
                tmp += 1;
                init_zero.push(format!("    LFSR 0, 0x{b:03X}"));
                init_zero.push(format!("    MOVLW 0x{take:02X}"));
                init_zero.push(format!("{l}:"));
                init_zero.push("    CLRF 0xFEE,A".to_string());
                init_zero.push("    DECFSZ 0xFE8,F,A".to_string());
                init_zero.push(format!("    BRA {l}"));
                b += take as u32;
                n -= take;
            }
        }
        let mut init: Vec<String> = Vec::new();
        for g in &m.globals {
            // Const globals living in RAM copy their bytes down; a mutable
            // global with an initializer also needs its bytes written, or it
            // silently reads as zero (epic-cc#454). `irparse` keeps `bytes`
            // empty for a zero-initialized global, so there is nothing to
            // emit for one and no init cost is added.
            if addrs.contains_key(&g.name) && g.needs_ram_init() {
                let base = addrs[&g.name];
                for (i, b) in g.bytes.iter().enumerate() {
                    let addr = base + i as u16;
                    // A ref byte is the high or low half of a pointer
                    // VALUE: a RAM target resolves through `addrs` right
                    // here (RAM globals have no assembler label to
                    // resolve, epic-cc#443); a flash target (function or
                    // const table) keeps its link-time label literal
                    // (epic-cc#154).
                    if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                        match addrs.get(f) {
                            Some(&a) => {
                                let byte = if i % 2 == 0 {
                                    a & 0xFF
                                } else {
                                    (a >> 8) & 0xFF
                                };
                                init.push(format!("    MOVLW 0x{byte:02X}"));
                            }
                            None => {
                                let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                                init.push(format!("    MOVLW {lit}({f})"));
                            }
                        }
                    } else {
                        init.push(format!("    MOVLW 0x{b:02X}"));
                    }
                    // Access-bank check mirrors Gen::operand.
                    if addr <= access_bank_hi || addr >= PIC18_SFR_ACCESS_LO {
                        init.push(format!("    MOVWF 0x{addr:03X},A"));
                    } else {
                        let bsr = (addr >> 8) as u8;
                        init.push(format!("    MOVLB 0x{bsr:02X}"));
                        init.push(format!("    MOVWF 0x{addr:03X},B"));
                    }
                }
            }
        }
        out.push("__start:".to_string());
        locs.push(None);
        let zero_len = init_zero.len();
        out.extend(init_zero);
        locs.extend(std::iter::repeat(None).take(zero_len));
        let init_len = init.len();
        out.extend(init);
        locs.extend(std::iter::repeat(None).take(init_len));
        out.push("    call main".to_string());
        locs.push(None);
        out.push("    sleep".to_string());
        locs.push(None);
        // A simulator that runs past SLEEP (or a spurious wake with no
        // enabled source) must not execute the const tables that follow:
        // hang loudly here instead of running data as code (epic-cc#643).
        out.push("__halt:".to_string());
        locs.push(None);
        out.push("    BRA __halt".to_string());
        locs.push(None);
    }
    // the flash const: every `const` (flash) global becomes a `DB` table after the code,
    // before `end`. The bytes are the flat LE blob `irparse` decoded; the
    // table label is the TBLPTR base `LOW`/`HIGH`/`UPPER` resolve. No
    // chunking and no `.align`: PIC18's `TBLRD` addresses program memory
    // linearly (byte addresses, two bytes per word), so the 511-byte
    // `RETLW` ceiling of PIC14 stops existing here.
    for g in &m.globals {
        if !g.is_const {
            continue;
        }
        assert!(
            !g.bytes.is_empty(),
            "isel-pic18: const @{} has no table bytes",
            g.name
        );
        out.push(format!("{}:", g.name));
        locs.push(None);
        // Chunk the plain bytes 8 per line (the earlier layout); a ref
        // byte materializes its own line (see the ref match below).
        let mut chunk: Vec<String> = Vec::new();
        for (i, b) in g.bytes.iter().enumerate() {
            // A ref byte is the high or low half of a pointer VALUE: a
            // RAM target resolves through `addrs` right here (RAM
            // globals have no assembler label to resolve, epic-cc#443);
            // a flash target (function or const table) keeps its
            // link-time label literal (epic-cc#154).
            if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                if !chunk.is_empty() {
                    out.push(format!("    db {}", chunk.join(", ")));
                    locs.push(None);
                    chunk.clear();
                }
                match addrs.get(f) {
                    Some(&a) => {
                        let byte = if i % 2 == 0 {
                            a & 0xFF
                        } else {
                            (a >> 8) & 0xFF
                        };
                        out.push(format!("    db 0x{byte:02X}"));
                    }
                    None => {
                        let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                        out.push(format!("    db {lit}({f})"));
                    }
                }
                locs.push(None);
            } else {
                chunk.push(format!("0x{b:02X}"));
                if chunk.len() == 8 {
                    out.push(format!("    db {}", chunk.join(", ")));
                    locs.push(None);
                    chunk.clear();
                }
            }
        }
        if !chunk.is_empty() {
            out.push(format!("    db {}", chunk.join(", ")));
            locs.push(None);
        }
        out.push("".to_string());
        locs.push(None);
    }
    // gpasm requires the `end` directive (our own assembler tolerates its
    // absence); PIC14's `isel::select` emits it the same way.
    out.push("    end".to_string());
    locs.push(None);
    (out.join("\n") + "\n", locs)
}

/// The W cache's global-address exclusion set for the unit tests below:
/// they exercise emitters directly, with no module-level placement, so no
/// address is a global (epic-cc#502).
#[cfg(test)]
fn empty_global_addrs() -> &'static HashSet<u16> {
    static EMPTY: std::sync::OnceLock<HashSet<u16>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the shared label counter: `Gen::tmp` borrows one module counter
    /// so `fresh_label()` never repeats `tmp{n}` across functions in one
    /// output. A per-function counter duplicates `tmp0`, which fails to
    /// assemble once phi elimination emits real branch labels. The test builds
    /// two `Gen`s on one backing counter, mirroring `select()` per function.
    #[test]
    fn fresh_label_counter_is_shared_across_gens() {
        let m = ir::parse("fn f(void) ()\n  block entry:\n    ret void\n");
        let addrs: HashMap<String, u16> = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut tmp = 0u32;
        let l1 = {
            let mut g = Gen {
                m: &m,
                addrs: &addrs,
                resolved: &resolved,
                retval_lo: 0,
                access_bank_hi: 0x5F,
                bsr: None,
                fwd_join: HashMap::new(),
                bsr_dirty: false,
                exit_banks: &exits,
                fsr0_holds: None,
                pending_copies: Vec::new(),
                cur_func: "f",
                global_addrs: empty_global_addrs(),
                w_holds: None,
                isr: false,
                tmp: &mut tmp,
                cur_loc: None,
                out: Vec::new(),
                locs: Vec::new(),
            };
            g.fresh_label()
        };
        let l2 = {
            let mut g = Gen {
                m: &m,
                addrs: &addrs,
                resolved: &resolved,
                retval_lo: 0,
                access_bank_hi: 0x5F,
                bsr: None,
                fwd_join: HashMap::new(),
                bsr_dirty: false,
                exit_banks: &exits,
                fsr0_holds: None,
                pending_copies: Vec::new(),
                cur_func: "f",
                global_addrs: empty_global_addrs(),
                w_holds: None,
                isr: false,
                tmp: &mut tmp,
                cur_loc: None,
                out: Vec::new(),
                locs: Vec::new(),
            };
            g.fresh_label()
        };
        assert_eq!(l1, "tmp0");
        assert_eq!(l2, "tmp1", "a second Gen sharing the same backing counter must continue, not restart, the sequence");
    }
}

#[cfg(test)]
mod p3_gen_tests {
    use super::*;

    fn gen<'a>(
        m: &'a Module,
        addrs: &'a HashMap<String, u16>,
        resolved: &'a PtrResolution,
        exits: &'a HashMap<String, Option<u8>>,
        tmp: &'a mut u32,
    ) -> Gen<'a> {
        Gen {
            m,
            addrs,
            resolved,
            retval_lo: 0,
            access_bank_hi: 0x5F,
            bsr: None,
            fwd_join: HashMap::new(),
            bsr_dirty: false,
            exit_banks: exits,
            fsr0_holds: None,
            pending_copies: Vec::new(),
            cur_func: "main",
            global_addrs: empty_global_addrs(),
            w_holds: None,
            isr: false,
            tmp,
            cur_loc: None,
            out: Vec::new(),
            locs: Vec::new(),
        }
    }

    #[test]
    fn low_access_bank_needs_no_movlb() {
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let mut tmp = 0u32;
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut g = gen(&m, &addrs, &resolved, &exits, &mut tmp);
        assert_eq!(g.operand(0x05F), (0, 0x5F));
        assert!(g.out.is_empty(), "no MOVLB for the low access-bank range");
    }

    #[test]
    fn banked_gpr_range_needs_movlb() {
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let mut tmp = 0u32;
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut g = gen(&m, &addrs, &resolved, &exits, &mut tmp);
        assert_eq!(g.operand(0x0090), (1, 0x90));
        assert!(
            g.out.iter().any(|l| l.contains("MOVLB")),
            "the banked range needs a MOVLB"
        );
    }

    #[test]
    fn forward_join_restores_agreement() {
        // Two recorded edges and the fall-through all hold bank 2: the
        // label restores it instead of resetting. Guards the meet
        // against regressions that drop the fall-through candidate.
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let mut tmp = 0u32;
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut g = gen(&m, &addrs, &resolved, &exits, &mut tmp);
        g.bsr = Some(2);
        g.note_branch("tmp7");
        g.note_branch("tmp7");
        g.emit_label("tmp7");
        assert_eq!(g.bsr, Some(2), "agreeing join must restore the bank");
    }

    #[test]
    fn forward_join_poison_on_disagreement() {
        // Recorded edges disagree (or the fall-through does): the label
        // stays unknown so the next banked access re-selects. Guards
        // the meet against regressions that overwrite instead of
        // poisoning, which would elide a needed MOVLB.
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let mut tmp = 0u32;
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut g = gen(&m, &addrs, &resolved, &exits, &mut tmp);
        g.bsr = Some(0);
        g.note_branch("tmp8");
        g.bsr = Some(1);
        g.emit_label("tmp8");
        assert_eq!(g.bsr, None, "divergent join must stay unknown");
        g.bsr = Some(1);
        g.note_branch("tmp9");
        g.bsr = None;
        g.emit_label("tmp9");
        assert_eq!(g.bsr, None, "unknown fall-through must poison");
    }

    #[test]
    fn sfr_high_segment_needs_no_movlb() {
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: PtrResolution = HashMap::new();
        let mut tmp = 0u32;
        let exits: HashMap<String, Option<u8>> = HashMap::new();
        let mut g = gen(&m, &addrs, &resolved, &exits, &mut tmp);
        // FSR0L, the address this lowering exists to fix.
        assert_eq!(
            g.operand(0xFE9),
            (0, 0xE9),
            "the SFR segment is access-bank, a=0"
        );
        assert!(
            g.out.is_empty(),
            "no MOVLB for an SFR address, regardless of the tracked BSR"
        );
    }

    #[test]
    fn exit_bank_joins_return_ends() {
        assert_eq!(exit_bank(&[Some(2), Some(2)]), Some(2));
        assert_eq!(exit_bank(&[Some(1), Some(2)]), None);
        assert_eq!(exit_bank(&[None]), None);
        assert_eq!(exit_bank(&[]), None);
    }
}
