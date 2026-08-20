//! `isel-pic18` — instruction selection for the PIC18 integer spine (P2).
//! Same scope as `isel`'s milestones 2-6 (`Load`/`Store`/`Bin`(add/sub/
//! and/or/xor)/`Icmp`/`Zext`/`Sext`/`Trunc`/`Select`/`Call`/`Br`/`BrCond`/
//! `Phi`/`Ret`) — see docs/superpowers/plans/2026-08-18-pic18-port-p2.md.
//! A separate crate from `isel` per docs/29-pic18-port-design.md §2 D-1:
//! the instruction sets differ enough that sharing would leak an
//! abstraction, and PIC14's working code must never be at risk from a
//! PIC18 edit.

use std::collections::{HashMap, HashSet};

use device::Device;
use ir::{Func, Inst, Module, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};

/// The result of resolving a pointer to a concrete access. `Direct`: the
/// address is statically known, so a plain `MOVFF`/`MOVF`/`MOVWF` reaches
/// it. `Indirect`: `FSR0` has been set up and the access goes through
/// `INDF0`.
enum Addr {
    Direct(u16),
    Indirect,
}

struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)` by `iselcore::resolve_pointers`; see
    /// docs/superpowers/plans/2026-08-20-pic18-port-p3.md Task 1/3.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    /// Fixed, `BSR`-independent return-value region (up to 4 bytes, from
    /// `device.common_ram`) — see the plan's "Where the retval/scratch
    /// design comes from" section.
    retval_lo: u16,
    /// The `BSR` value the last-emitted `MOVLB` set, or `None` when it's
    /// unknown (module start, or just after a label — branch targets can
    /// be reached with any prior `BSR` state, so it must be re-established
    /// on the next banked access rather than assumed).
    bsr: Option<u8>,
    cur_func: &'m str,
    /// Module-scoped fresh-label counter, shared across every function so
    /// the emitted `tmp{n}:` labels stay unique in the single `.asm`
    /// output (mirrors `isel`'s `tmp` field, `crates/isel/src/lib.rs:177`).
    tmp: &'m mut u32,
    out: Vec<String>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    /// Emit a label line AND reset the tracked `BSR` (`self.bsr = None`).
    /// Every label in this file — a real block label, a fresh
    /// `Select`/`Icmp` branch target, or a synthesized phi-copy label —
    /// is a place code from more than one preceding path can land, and
    /// each of those paths may have executed a different subset of the
    /// `MOVLB`s that led here (or none at all). `operand()`'s `MOVLB`
    /// elision is only sound when `self.bsr` reflects what's ACTUALLY
    /// true on every path reaching the current point, so any label must
    /// reset it — trusting a stale tracked value across a branch target
    /// has been the exact root cause of three separate miscompile bugs
    /// found across this task's review rounds (`Select`'s `l_else` and
    /// `l_end`, `BrCond`'s synthesized `l_fcopies`, and — narrower, but
    /// the same class — `emit_icmp_i16`'s shared `l_true`/`l_false`).
    /// This helper makes the reset structural instead of a fact every
    /// label call site has to individually remember: every
    /// `self.emit(format!("{{...}}:"))`/`g.emit(format!("{{...}}:"))` in
    /// this file routes through this instead.
    ///
    /// Labels are not the ONLY BSR-clobbering join point, though: a
    /// `CALL` return is another one (the callee runs its own arbitrary
    /// `MOVLB`s and never restores the caller's bank on `RETURN`), but it
    /// is not itself a label, so it structurally cannot go through this
    /// helper — `Inst::Call`'s arm in `emit_inst` resets `self.bsr`
    /// directly right after emitting `CALL`.
    fn emit_label(&mut self, label: &str) {
        self.emit(format!("{label}:"));
        self.bsr = None;
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

    /// The folded `(base, k, terms)` for pointer reg `r` in the current
    /// function, from the module-wide `resolve_pointers` map. A pointer
    /// with no entry is a bug in an earlier stage (every `gep` result and
    /// every byval/sret/alloca seed must have been resolved), so it
    /// panics loudly rather than silently emitting a bogus access.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel-pic18: no gep for pointer %{r} ({key})"))
    }

    /// The byte size of the object a resolved pointer's base ultimately
    /// refers to: a global's declared size, or a slot's (an alloca's
    /// buffer size, or a byval/sret param's declared width). Used to size
    /// a struct-copy loop (byval call args, sret returns; Tasks 7-8).
    ///
    /// Unlike PIC14's `object_span`, this has NO window-fit check baked
    /// in: PIC18's flat FSR addressing has no window an object could
    /// straddle (see this plan's Global Constraints). This function only
    /// ever answers "how many bytes," never "does it fit."
    fn object_span(&self, base: &Base) -> u16 {
        match base {
            Base::Global(name) => self
                .m
                .globals
                .iter()
                .find(|g| g.name == *name)
                .unwrap_or_else(|| panic!("isel-pic18: unknown global @{name}"))
                .size as u16,
            Base::Slot(sname, _) => {
                let f = self
                    .m
                    .funcs
                    .iter()
                    .find(|f| f.name == self.cur_func)
                    .unwrap_or_else(|| panic!("isel-pic18: no span for slot {sname}: unknown function {}", self.cur_func));
                if let Some(p) = f.params.iter().find(|p| p.name == *sname) {
                    p.width as u16
                } else if let Some(a) = f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| match i {
                    Inst::Alloca(a) if a.dst == *sname => Some(a.size),
                    _ => None,
                }) {
                    a as u16
                } else {
                    panic!("isel-pic18: no span for slot {sname} in {}", self.cur_func)
                }
            }
        }
    }

    /// The `,A`/`,B` operand components `(a, f)` for a physical address
    /// used by a `W`-routing instruction (`ADDWF`/`SUBWF`/.../`CPFSxx`/
    /// `MOVWF`), emitting `MOVLB` first if the tracked `BSR` doesn't
    /// already match. PIC18's Access Bank is TWO disjoint ranges: the low
    /// general-purpose segment (`0x000-0x05F`) and the high SFR segment
    /// (`0xF60-0xFFF` [VERIFY against DS39632], every SFR, including
    /// FSR0L/FSR0H/FSR1L/FSR1H/FSR2L/FSR2H, lives here), BOTH always
    /// reachable via `a=0` with no `MOVLB`, regardless of `BSR`. Only the
    /// MIDDLE range (`0x060-0xF5F`, ordinary banked GPR) needs a bank
    /// select. Getting this wrong for the SFR segment would emit a
    /// `MOVLB` before every FSR-setup write and address FSRnL/FSRnH as
    /// `,B`, architecturally wrong; hardware requires `,A` for the SFR
    /// segment unconditionally.
    ///
    /// `MOVFF`-based plain copies (`emit_copy_byte`, above) never call
    /// this: they take full 12-bit addresses directly and need no bank
    /// bit at all, which is why `Load`/`Store`/`Phi`-copies/`Call`-arg-
    /// copies never touch `BSR`.
    fn operand(&mut self, addr: u16) -> (u16, u16) {
        if addr < 0x60 || addr >= 0xF60 {
            (0, addr & 0xFF)
        } else {
            let bank = (addr >> 8) as u8;
            if self.bsr != Some(bank) {
                self.emit(format!("    MOVLB 0x{bank:X}"));
                self.bsr = Some(bank);
            }
            (1, addr & 0xFF)
        }
    }

    /// One byte, memory-to-memory, via `MOVFF` — no access bit, no `BSR`.
    fn emit_copy_byte(&mut self, src: u16, dst: u16) {
        self.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
    }

    /// Copy `val` (width `ty.bytes()`) into the slot starting at `dst`. A
    /// register/global source uses `MOVFF` (no access bit needed); a
    /// constant has no `MOVFF` literal form, so it goes through `W` via
    /// `MOVLW`/`MOVWF` (which DOES need the access bit — this is the one
    /// place a plain copy still touches `operand`/`BSR`).
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u16) {
        match val {
            Val::Const(k) => {
                for i in 0..ty.bytes() {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVLW 0x{byte:02X}"));
                    let (a, f) = self.operand(dst + u16::from(i));
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                }
            }
            _ => {
                let src = self.val_addr(val).direct();
                for i in 0..ty.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
            }
        }
    }

    /// How a byte access at `ptr + byte_off` completes. `Val::Global`
    /// resolves directly (globals are never behind another indirection
    /// layer). `Val::Reg` looks up the pointer's fold via `resolved_for`:
    /// a `Base::Global`/`Base::Slot(_, false)` (byval/alloca, the slot
    /// itself IS the object) with no dynamic terms is a plain direct
    /// address; with dynamic terms it needs FSR0 (Task 6). A
    /// `Base::Slot(_, true)` (sret, the slot holds a target ADDRESS, not
    /// the object) always needs FSR0, regardless of terms (Task 7).
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => Addr::Direct(self.global_addr(g) + u16::from(byte_off)),
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        if terms.is_empty() {
                            Addr::Direct(self.global_addr(name) + u16::from(k) + u16::from(byte_off))
                        } else {
                            self.emit_fsr0_dynamic(self.global_addr(name), k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        if !indirect && terms.is_empty() {
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
                        } else if !indirect {
                            self.emit_fsr0_dynamic(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else {
                            self.emit_fsr0_indirect_slot(sa, k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel-pic18: pointer operand must be a register or global"),
        }
    }

    /// Set `FSR0 = base_addr + k + Σ scale×%reg + byte_off` and leave the
    /// access to go through `INDF0` (0xFEF). `LFSR` seeds the STATIC part
    /// (`base_addr + k + byte_off`, all known at codegen time: one
    /// two-word instruction, no addressing-mode complexity at all); the
    /// dynamic term, if any, is then added onto `FSR0L`/`FSR0H` with
    /// carry via `ADDWF`/`ADDWFC` (both routed through `operand()`, which
    /// Task 2 taught to recognize FSR0L/FSR0H as the always-access-bank
    /// SFR segment, so no `MOVLB` is ever emitted for these two writes).
    ///
    /// `terms.len() > 1` is out of scope for P3 (see this plan's Scope
    /// boundary) and panics loudly rather than silently dropping a term.
    fn emit_fsr0_dynamic(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P3 scope; {} terms)",
            terms.len()
        );
        let static_part = u16::from(k) + u16::from(byte_off);
        let lit = (base_addr + static_part) & 0xFFF;
        self.emit(format!("    LFSR 0, 0x{lit:03X}"));
        if let Some((scale, reg)) = terms.first() {
            let a = self.slot_addr(self.cur_func, reg).direct();
            for _ in 0..*scale {
                let (ra, rf) = self.operand(a);
                self.emit(format!("    MOVF 0x{rf:03X},W,{}", if ra == 0 { "A" } else { "B" }));
                let (fa, ff) = self.operand(0xFE9); // FSR0L
                self.emit(format!("    ADDWF 0x{ff:03X},F,{}", if fa == 0 { "A" } else { "B" }));
                self.emit("    MOVLW 0x00".to_string());
                let (ha, hf) = self.operand(0xFEA); // FSR0H
                self.emit(format!("    ADDWFC 0x{hf:03X},F,{}", if ha == 0 { "A" } else { "B" }));
            }
        }
    }
    fn emit_fsr0_indirect_slot(&mut self, _slot_addr: u16, _k: u8, _terms: &[(u8, String)], _byte_off: u8) {
        panic!("isel-pic18: sret-indirect pointer offsets arrive in Task 7");
    }

    /// Materialize byte `k` of `val` into `W`: a constant via `MOVLW`, a
    /// register/global source via `MOVF`. Used only by the
    /// `Addr::Indirect` store path (the direct path uses
    /// `emit_move_val_to_slot`/`MOVFF`, which never touches `W`).
    fn emit_byte_to_w(&mut self, val: &Val, k: u8) {
        match val {
            Val::Const(c) => {
                let byte = ((c >> (i64::from(k) * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{byte:02X}"));
            }
            Val::Reg(r) => {
                let a = self.slot_addr(self.cur_func, r).direct() + u16::from(k);
                let (ab, f) = self.operand(a);
                self.emit(format!("    MOVF 0x{f:03X},W,{}", if ab == 0 { "A" } else { "B" }));
            }
            Val::Global(g) => {
                let a = self.global_addr(g) + u16::from(k);
                let (ab, f) = self.operand(a);
                self.emit(format!("    MOVF 0x{f:03X},W,{}", if ab == 0 { "A" } else { "B" }));
            }
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel-pic18: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                let ptr_val = if let Some(g) = l.ptr.strip_prefix('@') {
                    Val::Global(g.to_string())
                } else if let Some(r) = l.ptr.strip_prefix('%') {
                    Val::Reg(r.to_string())
                } else {
                    panic!("isel-pic18: malformed load pointer operand {:?}", l.ptr);
                };
                for k in 0..l.ty.bytes() {
                    match self.emit_ptr_setup(&ptr_val, k) {
                        Addr::Direct(src) => self.emit_copy_byte(src, dst + u16::from(k)),
                        Addr::Indirect => self.emit(format!("    MOVFF 0xFEF, 0x{:03X}", dst + u16::from(k))),
                    }
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel-pic18: only i8/i16 stores supported");
                let ptr_val = if let Some(g) = s.ptr.strip_prefix('@') {
                    Val::Global(g.to_string())
                } else if let Some(r) = s.ptr.strip_prefix('%') {
                    Val::Reg(r.to_string())
                } else {
                    panic!("isel-pic18: malformed store pointer operand {:?}", s.ptr);
                };
                // Direct case: a single emit_ptr_setup(_, 0) covers the
                // whole value via emit_move_val_to_slot (unchanged from
                // P2 for the common @global case). Indirect case:
                // re-resolve per byte (each byte's FSR setup is
                // independent; see Task 6's design note on why no
                // auto-increment is used), materializing the source byte
                // into W (a constant literally, a register via MOVF) and
                // writing it through INDF0.
                match self.emit_ptr_setup(&ptr_val, 0) {
                    Addr::Direct(dst) => self.emit_move_val_to_slot(&s.val, s.ty, dst),
                    Addr::Indirect => {
                        for k in 0..s.ty.bytes() {
                            if k > 0 {
                                self.emit_ptr_setup(&ptr_val, k);
                            }
                            self.emit_byte_to_w(&s.val, k);
                            self.emit("    MOVWF 0xFEF,A".to_string()); // INDF0
                        }
                    }
                }
            }
            Inst::Bin(b) => {
                let n = b.ty.bytes();
                assert!(n == 1 || n == 2, "isel-pic18: only i8/i16 Bin ops implemented (n={n})");
                // `b.a` is resolved via `val_addr`, which treats a
                // `Val::Const` as a RAM ADDRESS (`Slot::Direct(k & 0xFF)`)
                // rather than a literal to load — a constant on the LHS
                // (e.g. `sub i8 5, %x`, which clang can emit directly from
                // `5 - x`, and which the differential fuzzer generates) would
                // silently read whatever byte lives at that address instead
                // of using the literal. `b.b`'s RHS is fine: it always goes
                // through `emit_load_w`, which loads a `Val::Const` via
                // `MOVLW`. This mirrors the const-LHS hazard PIC14's `isel`
                // already had to guard against (see `emit_sub_const_lhs`,
                // `crates/isel/src/lib.rs:1598-1609`, and `emit_commutative`,
                // `crates/isel/src/lib.rs:1100-1106`) — full canonicalization
                // isn't this task's job, so fail loudly instead of
                // miscompiling silently, until a later task adds it.
                assert!(
                    !matches!(b.a, Val::Const(_)),
                    "isel-pic18: const-LHS Bin (constant as the first operand) not yet supported — needs the isel::emit_sub_const_lhs-equivalent handling"
                );
                let av = self.val_addr(&b.a).direct();
                let dst = self.slot_addr(self.cur_func, &b.dst).direct();
                for i in 0..n {
                    // SUBWF computes f - W; the IR's `sub a, b` is `a - b`,
                    // so `a` must be `f` and `b` must go into `W` first.
                    self.emit_load_w(&b.b, i);
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
                        (other, _) => panic!("isel-pic18: Bin op {other:?} not yet implemented (Task 6+)"),
                    };
                    let (aacc, af) = self.operand(av + u16::from(i));
                    let abank = if aacc == 0 { "A" } else { "B" };
                    self.emit(format!("    {mne} 0x{af:03X},W,{abank}"));
                    let (dacc, df) = self.operand(dst + u16::from(i));
                    let dbank = if dacc == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{df:03X},{dbank}"));
                }
            }
            Inst::Icmp(c) => {
                let n = c.ty.bytes();
                assert!(n == 1 || n == 2, "isel-pic18: only i8/i16 Icmp implemented so far (n={n})");
                if n == 1 {
                    self.emit_icmp_byte(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
                } else {
                    self.emit_icmp_i16(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
                }
            }
            Inst::Zext(z) => {
                // `val_addr` maps `Val::Const(k)` to a RAM ADDRESS
                // (`k & 0xFF`), not a literal — same hazard already guarded
                // for `Bin`/`Icmp` above. Not known to be reachable from
                // clang-generated IR or the differential fuzzer (both only
                // ever cast a loaded/computed register, never a bare
                // literal — a literal cast is constant-foldable by the
                // frontend before it ever reaches this backend), but the
                // cheap guard costs nothing and keeps a future const-source
                // producer from silently miscompiling instead of panicking.
                assert!(
                    !matches!(z.val, Val::Const(_)),
                    "isel-pic18: const source Zext not yet supported"
                );
                // Mirrors `isel`'s own width guard (`crates/isel/src/lib.rs`,
                // "isel: zext must not narrow") — without it, a malformed
                // `Zext` with `to.bytes() < from.bytes()` would copy
                // `from.bytes()` bytes into a narrower `to.bytes()` slot
                // below, writing past the destination into whatever local
                // sits next to it. Equal widths (e.g. `zext i1 to i8`,
                // where both types report `.bytes() == 1` in the byte
                // model — an icmp result is materialized as a byte holding
                // exactly 0/1, so a 1-byte copy IS the zext) are legal and
                // common (`u8 b = (a < b);`) and must be accepted: the
                // "extra high bytes" loop from `from.bytes()..to.bytes()`
                // below simply does not execute when the widths are equal.
                // Not reachable from today's clang-generated IR for the
                // narrowing case, but a real asymmetry with `isel`
                // otherwise (silent corruption here vs. a clean panic
                // there).
                assert!(
                    z.to.bytes() >= z.from.bytes(),
                    "isel-pic18: zext must not narrow"
                );
                let src = self.val_addr(&z.val).direct();
                let dst = self.slot_addr(self.cur_func, &z.dst).direct();
                for i in 0..z.from.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit("    MOVLW 0x00".to_string());
                    let (a, f) = self.operand(dst + u16::from(i));
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                }
            }
            Inst::Sext(s) => {
                // Same const-source hazard as `Inst::Zext`, see its comment.
                assert!(
                    !matches!(s.val, Val::Const(_)),
                    "isel-pic18: const source Sext not yet supported"
                );
                // Same width-relationship hazard as `Inst::Zext` above,
                // mirroring `isel`'s own sext bounds guard
                // (`crates/isel/src/lib.rs`, "isel: sext only supports
                // i8/i16 -> i16/i32"): without it, `to.bytes() <=
                // from.bytes()` would sign-fill zero or a negative number
                // of high bytes and still have copied `from.bytes()` bytes
                // into a same-or-narrower `to.bytes()` slot, writing past
                // the destination.
                assert!(
                    s.to.bytes() > s.from.bytes(),
                    "isel-pic18: sext must widen (to must be strictly wider than from)"
                );
                let src = self.val_addr(&s.val).direct();
                let dst = self.slot_addr(self.cur_func, &s.dst).direct();
                for i in 0..s.from.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
                // The sign-fill byte(s) must reflect the SOURCE's actual
                // sign bit (bit 7 of its highest byte) at the time this
                // cast runs — not an assumption. `MOVLW 0x00` first, then
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
                // Mirrors `isel`'s own width guard (`crates/isel/src/lib.rs`,
                // "isel: trunc must narrow") — without it, a malformed
                // `Trunc` with `to.bytes() >= from.bytes()` would read
                // `to.bytes()` bytes starting at `src` below, past the end
                // of the (narrower) source slot, and copy that overrun into
                // the destination.
                assert!(
                    t.to.bytes() < t.from.bytes(),
                    "isel-pic18: trunc must narrow (to must be strictly smaller than from)"
                );
                let src = self.val_addr(&t.val).direct();
                let dst = self.slot_addr(self.cur_func, &t.dst).direct();
                for i in 0..t.to.bytes() {
                    self.emit_copy_byte(src + u16::from(i), dst + u16::from(i));
                }
            }
            Inst::Select(s) => {
                // `a`/`b` route through `emit_move_val_to_slot`, which
                // handles `Val::Const` correctly (via `MOVLW`+`MOVWF`, not
                // as a RAM address through `val_addr`) and never branches
                // on a flag — no guard needed for either.
                //
                // `cond` is different: it's loaded via `emit_load_w`, then
                // immediately tested with `BZ`, which relies on the LOAD
                // having set the Z flag from `cond`'s value. That's true
                // when `cond` is `Val::Reg`/`Val::Global` (`emit_load_w`
                // emits `MOVF ...,W`, and this project's simulator's MOVF
                // calls `set_zn` — `crates/sim/src/lib.rs:779-783`). It is
                // NOT true for `Val::Const`: `emit_load_w`'s const arm
                // emits only `MOVLW`, and the simulator's MOVLW (PIC18
                // opcode 0xE, `crates/sim/src/lib.rs:903`) does `self.w = k`
                // with no `set_zn` call at all — so `BZ` would test
                // whatever Z flag the PREVIOUS instruction happened to
                // leave, silently picking the wrong side of the `Select`.
                // Same hazard class as the const-LHS/const-source guards
                // elsewhere in this file; guard it the same way.
                assert!(
                    !matches!(s.cond, Val::Const(_)),
                    "isel-pic18: const cond Select not yet supported"
                );
                let dst = self.slot_addr(self.cur_func, &s.dst).direct();
                let l_else = self.fresh_label();
                let l_end = self.fresh_label();
                self.emit_load_w(&s.cond, 0);
                self.emit(format!("    BZ {l_else}")); // cond byte == 0 -> else
                self.emit_move_val_to_slot(&s.a, s.ty, dst);
                self.emit(format!("    BRA {l_end}"));
                self.emit_label(&l_else);
                self.emit_move_val_to_slot(&s.b, s.ty, dst);
                self.emit_label(&l_end);
            }
            Inst::Call(c) => {
                let callee = self
                    .m
                    .funcs
                    .iter()
                    .find(|f| f.name == c.func)
                    .unwrap_or_else(|| panic!("isel-pic18: call to unknown function @{}", c.func));
                for (i, arg) in c.args.iter().enumerate() {
                    let pname = &callee.params[i].name;
                    let pa = self.slot_addr(&c.func, pname).direct();
                    if let Some(size) = arg.byval {
                        // `byval` means "copy `size` bytes from the address
                        // `arg.val` points to" — `val_addr` is the right
                        // resolver for an address-valued `Val`, but its
                        // `Val::Const` arm treats the constant itself as a
                        // RAM address (`k & 0xFF`), same hazard class as
                        // `Bin`'s LHS/the cast ops' sources above. A byval
                        // arg is always meant to be a pointer (an alloca's
                        // address, or another byval slot's), so a bare
                        // integer literal here has no sensible meaning —
                        // fail loudly rather than silently copy from
                        // whatever byte happens to live at `k & 0xFF`.
                        assert!(
                            !matches!(arg.val, Val::Const(_)),
                            "isel-pic18: const byval call arg not yet supported"
                        );
                        let src = self.val_addr(&arg.val).direct();
                        for b in 0..size {
                            self.emit_copy_byte(src + u16::from(b), pa + u16::from(b));
                        }
                    } else if arg.sret {
                        // `sret` means "store the 2-byte ADDRESS `arg.val`
                        // points to into the callee's sret slot" — same
                        // const hazard as `byval` above: an sret arg is
                        // always meant to be a pointer, so a literal here
                        // has no sensible meaning.
                        assert!(
                            !matches!(arg.val, Val::Const(_)),
                            "isel-pic18: const sret call arg not yet supported"
                        );
                        let addr = self.val_addr(&arg.val).direct();
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        let (a0, f0) = self.operand(pa);
                        self.emit(format!("    MOVWF 0x{f0:03X},{}", if a0 == 0 { "A" } else { "B" }));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        let (a1, f1) = self.operand(pa + 1);
                        self.emit(format!("    MOVWF 0x{f1:03X},{}", if a1 == 0 { "A" } else { "B" }));
                    } else {
                        let ty = arg.ty.expect("isel-pic18: scalar call arg must carry a type");
                        self.emit_move_val_to_slot(&arg.val, ty, pa);
                    }
                }
                self.emit(format!("    CALL {}", c.func));
                // A `CALL` return is a BSR-clobbering join point, same
                // reasoning as `emit_label` (see its doc comment), but it
                // is NOT itself a label, so `emit_label`'s reset doesn't
                // cover it: the callee runs its own arbitrary sequence of
                // `MOVLB`s and never restores the caller's bank on
                // `RETURN`, so `self.bsr` (which tracks the bank the MOST
                // RECENT `MOVLB` set) is stale the instant control returns
                // here. Trusting it would make `operand()` wrongly elide a
                // needed `MOVLB` on the next banked access after the call,
                // silently reading/writing the wrong physical address.
                self.bsr = None;
                if let Some(d) = &c.dst {
                    let ty = c.ty.expect("isel-pic18: valued call must carry a type");
                    let dst = self.slot_addr(self.cur_func, d).direct();
                    for i in 0..ty.bytes() {
                        self.emit_copy_byte(self.retval_lo + u16::from(i), dst + u16::from(i));
                    }
                }
            }
            Inst::Gep(_) => {
                // Virtual: Gep's result is folded away by `resolve_pointers`
                // (Task 1) before codegen ever runs; the folded `(base, k,
                // terms)` is consumed at the point of use (`Load`/`Store`/
                // `Memcpy`/byval-sret) via `Gen::resolved_for`. It emits
                // nothing of its own. (Alloca gets the same treatment in
                // Task 10.)
            }
            other => panic!("isel-pic18: unsupported instruction for P2 (so far): {other:?}"),
        }
    }

    /// `dst = (a <pred> b) ? 1 : 0` for one byte, via `a - b` (SUBWF: f=a,
    /// W=b beforehand, d=W so `a`'s slot is untouched) and a flag-based
    /// branch. C/Z/N/OV follow PIC18's standard (ARM-style) condition-code
    /// semantics — C=1 means "no borrow" (a>=b unsigned) — already relied
    /// on by P1's `Pic18::sub_flags`.
    ///
    /// Delegates the actual flag test to `emit_cmp_branch` (shared with
    /// the i16 path, Task 9). A single byte has no "next byte" to defer
    /// to, so the "equal" outcome must resolve directly to this
    /// predicate's real answer at equality: true for the non-strict/eq
    /// predicates (`eq`, `uge`, `ule`, `sge`, `sle`), false for the
    /// strict ones (`ne`, `ult`, `ugt`, `slt`, `sgt`) — NOT uniformly
    /// `l_false`, which would silently invert `uge`/`ule`/`sge`/`sle` at
    /// equality (e.g. `ule(5, 5)` must stay `1`).
    fn emit_icmp_byte(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // `val_addr` maps `Val::Const(k)` to a RAM ADDRESS (`k & 0xFF`),
        // not a literal — a constant on the LHS (e.g. `icmp ult i8 5, %x`)
        // would silently compare against whatever byte lives at address
        // 0x05 instead of the literal 5. Same hazard as `Inst::Bin`'s LHS
        // (see the `assert!` there); fail loudly until a later task adds
        // real const-LHS canonicalization.
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
        self.emit_cmp_branch(&a, &b, 0, pred, &l_true, &l_false, &l_equal);
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst);
    }

    /// `dst = (a <pred> b) ? 1 : 0` for two bytes: compare the high byte
    /// (offset 1) first, with `pred`'s own signedness — if it differs,
    /// that alone decides the whole 16-bit result, since the sign only
    /// ever lives in the most-significant byte. Only when the high bytes
    /// are equal does the low byte (offset 0) get compared, always
    /// **unsigned** (`slt`->`ult`, `sle`->`ule`, `sgt`->`ugt`, `sge`->`uge`;
    /// `ult`/`ule`/`ugt`/`uge` already are their own tie-break).
    ///
    /// `eq`/`ne` don't fit this "high byte decides" shape at all — they
    /// need BOTH bytes equal (`eq`) or EITHER byte different (`ne`), so
    /// they're dispatched to their own short-circuit, `emit_icmp_i16_eq_ne`,
    /// instead of the tie-break machinery built for the eight ordering
    /// predicates.
    fn emit_icmp_i16(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // Same const-LHS hazard as `emit_icmp_byte` — this is a separate
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

        // High byte, `pred`'s own signedness. Equal high bytes never
        // decide the outcome by themselves (a lower byte could still flip
        // the overall order either way) so "equal" always defers to the
        // low-byte tie-break, regardless of predicate.
        self.emit_cmp_branch(&a, &b, 1, pred, &l_true, &l_false, &l_check_low);
        self.emit_label(&l_check_low);
        // Low byte, unsigned tie-break. Here "equal" means the two
        // 16-bit values are fully identical, so — unlike the high byte —
        // it DOES have a final answer: true for the non-strict tie-break
        // predicates (`ule`/`uge`), false for the strict ones
        // (`ult`/`ugt`).
        let l_low_equal = if matches!(unsigned_tiebreak, "ule" | "uge") {
            l_true.clone()
        } else {
            l_false.clone()
        };
        self.emit_cmp_branch(&a, &b, 0, unsigned_tiebreak, &l_true, &l_false, &l_low_equal);
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst);
    }

    /// `eq`/`ne` for i16: true (for `eq`) only when both bytes match;
    /// `ne` is the mirror. Two direct byte-equality checks (`SUBWF` +
    /// `BNZ`), independent of the signed/unsigned tie-break machinery
    /// `emit_icmp_i16` uses for the eight ordering predicates — a partial
    /// match (high byte equal, low byte different) is decisive here in a
    /// way it never is for `slt`/`ult`/etc.
    fn emit_icmp_i16_eq_ne(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        let l_true = self.fresh_label();
        let l_false = self.fresh_label();
        let l_done = self.fresh_label();
        let l_mismatch = if pred == "eq" { &l_false } else { &l_true };
        for offset in 0..2u8 {
            self.emit_load_w(&b, offset);
            let av = self.val_addr(&a).direct() + u16::from(offset);
            let (acc, af) = self.operand(av);
            let bank = if acc == 0 { "A" } else { "B" };
            self.emit(format!("    SUBWF 0x{af:03X},W,{bank}")); // W = a - b
            self.emit(format!("    BNZ {l_mismatch}"));
        }
        // Every byte matched: `eq` is true, `ne` is false.
        let l_all_matched = if pred == "eq" { &l_true } else { &l_false };
        self.emit(format!("    BRA {l_all_matched}"));
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst);
    }

    /// The shared flag-test core behind `emit_icmp_byte`/`emit_icmp_i16`:
    /// computes `a - b` (SUBWF, `W = a - b`) for the byte at `byte_offset`
    /// and branches on `pred`'s C/Z/N/OV condition — three ways, not two:
    /// `l_true` if `pred` holds for this byte pair, `l_false` if it
    /// definitely does not (the bytes differ in the "wrong" direction),
    /// or `l_equal` if the two bytes are equal. Equality is inherently
    /// ambiguous from a single byte's flags alone — the caller decides
    /// what it means: "go check the next byte" (i16's high-byte compare),
    /// or "that IS the final answer" (i8, and i16's low-byte tie-break),
    /// by choosing what `l_equal` points at.
    ///
    /// `eq`/`ne` are exempt from the three-way split — a byte's equality
    /// already IS their complete per-byte answer, so their arms use only
    /// `l_true`/`l_false`. i16's `Icmp` lowering never calls this for
    /// `eq`/`ne` (see `emit_icmp_i16_eq_ne`); i8's `emit_icmp_byte` does,
    /// with `l_equal` bound to whichever of `l_true`/`l_false` matches.
    fn emit_cmp_branch(&mut self, a: &Val, b: &Val, byte_offset: u8, pred: &str, l_true: &str, l_false: &str, l_equal: &str) {
        assert!(
            !matches!(a, Val::Const(_)),
            "isel-pic18: const-LHS Icmp (constant as the first operand) not yet supported"
        );
        self.emit_load_w(b, byte_offset);
        let av = self.val_addr(a).direct() + u16::from(byte_offset);
        let (acc, af) = self.operand(av);
        let bank = if acc == 0 { "A" } else { "B" };
        self.emit(format!("    SUBWF 0x{af:03X},W,{bank}")); // W = a - b

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
    /// `emit_icmp_i16_eq_ne` — the only difference between the three is
    /// how they arrive at `l_true`/`l_false`.
    fn emit_materialize_bool(&mut self, l_true: &str, l_false: &str, l_done: &str, dst: &str) {
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

    /// Load byte `offset` of any `Val` into `W` — a constant via `MOVLW`
    /// (shifting the literal right by `offset*8` bytes first), a
    /// register/global via `MOVF ...,W` at the resolved address plus
    /// `offset` (which needs the access bit, same as any other
    /// `W`-routing instruction).
    fn emit_load_w(&mut self, v: &Val, offset: u8) {
        match v {
            Val::Const(k) => {
                let byte = ((*k >> (u32::from(offset) * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{byte:02X}"));
            }
            _ => {
                let addr = self.val_addr(v).direct() + u16::from(offset);
                let (a, f) = self.operand(addr);
                let bank = if a == 0 { "A" } else { "B" };
                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
            }
        }
    }
}

/// The classic iterative dominator sets for a function's CFG: `doms[b]` is
/// the set of blocks that dominate block `b`. Used to classify phi-copy
/// edges: `pred -> merge` is a BACK edge iff `merge` dominates `pred` — the
/// pred is inside the merge's loop, so on that edge the merge's phi slots
/// hold the CURRENT iteration's values. Ported from `isel`'s own
/// `block_dominators` (`crates/isel/src/lib.rs:3977-4022`) — that function
/// is private to the `isel` crate (not `pub`), and this task's scope is
/// limited to `isel-pic18`'s own files, so the algorithm is duplicated
/// here rather than shared. Covers self-loops (pred == merge) AND
/// separate-latch back edges (pred is a latch block).
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

/// Emit the dependency-ordered phi copies for one (pred -> merge) edge: a
/// copy never overwrites a slot a later copy still needs to read. Ported
/// from `isel`'s own `emit_phi_copies` (`crates/isel/src/lib.rs:4296-4345`,
/// private to that crate — same reason as `block_dominators` above).
///
/// The ordering depends on whether the edge is a BACK edge into the merge
/// (`back_edge`, from `block_dominators` — the merge dominates the pred):
/// - Back edge: the merge's phi slots hold the CURRENT iteration's values,
///   so a copy reading a slot another copy writes must run BEFORE the
///   overwrite (reader first).
/// - Forward edge: a phi slot is only defined by THIS edge's copies, so a
///   copy reading a slot another copy writes must run AFTER its definer
///   (writer first).
/// A true cycle (%a <- %b, %b <- %a) needs a temp register, so it panics
/// loudly rather than silently miscompile.
fn emit_phi_copies<'m>(g: &mut Gen<'m>, copies: &[(String, Ty, Val)], back_edge: bool) {
    let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
        .iter()
        .map(|(dst, ty, val)| {
            let da = g.slot_addr(g.cur_func, dst).direct();
            let src = match val {
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

pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    let (common_lo, _) = device
        .common_ram
        .expect("isel-pic18's fixed retval region needs a common-RAM reservation");
    let mut out = vec![
        "; pic8 -- P2 integer spine (isel-pic18)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    // Shared across every `Gen` below so `fresh_label()` never repeats a
    // `tmp{n}:` label across two different functions in the same output.
    let mut tmp = 0u32;
    // Every pointer reg in the module, folded once up front; later tasks'
    // pointer emitters consume it via `Gen::resolved_for`.
    let resolved = resolve_pointers(m);
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            resolved: &resolved,
            retval_lo: common_lo,
            bsr: None,
            cur_func: &f.name,
            tmp: &mut tmp,
            out: Vec::new(),
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
            let lbl = if i == 0 { f.name.clone() } else { format!("{}_L{}", f.name, b.label) };
            labels.insert(b.label.clone(), lbl);
        }
        // Phi elimination: for each (predecessor, merge) EDGE — not just
        // the predecessor — the copies that must run when that edge is
        // taken. Keying by predecessor alone (Task 12's original version)
        // is a real miscompile on a `BrCond` whose two successors both
        // consume phis: running both successors' copies unconditionally
        // before the branch clobbers a loop header's phi slot with the
        // next-iteration value before the exit edge's phi gets a chance to
        // read the CURRENT one. Ported from `isel`'s own scheme
        // (`crates/isel/src/lib.rs:4067-4086`).
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
        for b in &f.blocks {
            g.emit_label(&labels[&b.label]);
            let mut terminator: Option<&Inst> = None;
            for inst in &b.insts {
                match inst {
                    Inst::Phi(_) => {} // eliminated; copies emitted at pred ends
                    Inst::Br(_) | Inst::BrCond(_) | Inst::Ret(_) => terminator = Some(inst),
                    other => g.emit_inst(other),
                }
            }
            match terminator {
                Some(Inst::Br(br)) => {
                    let merge = br.target.clone();
                    if let Some(c) = phi_copies.get(&(b.label.clone(), merge.clone())) {
                        emit_phi_copies(&mut g, c, doms[&b.label].contains(&merge));
                    }
                    g.emit(format!("    BRA {}", labels[&merge]));
                }
                Some(Inst::BrCond(bc)) => {
                    // Same hazard class as `Select`'s cond (Task 11, see
                    // `emit_inst`'s `Inst::Select` arm): `emit_load_w`'s
                    // `Val::Const` arm emits only `MOVLW`, which does not
                    // set the Z flag (`crates/sim/src/lib.rs`), so a
                    // literal cond here would make `BZ` test a stale flag
                    // from whatever came before instead of `cond`'s real
                    // value. Fail loudly instead of silently miscompiling.
                    assert!(
                        !matches!(bc.cond, Val::Const(_)),
                        "isel-pic18: const cond BrCond not yet supported"
                    );
                    let lt = labels[&bc.t].clone();
                    let lf = labels[&bc.f].clone();
                    let t_copies = phi_copies.get(&(b.label.clone(), bc.t.clone())).cloned();
                    let f_copies = phi_copies.get(&(b.label.clone(), bc.f.clone())).cloned();
                    g.emit_load_w(&bc.cond, 0);
                    // Unlike PIC14's BTFSC/BTFSS (a 1-instruction SKIP, so a
                    // copy sequence longer than one instruction needs an
                    // extra `lcop` block to route through), PIC18's BZ/BNZ
                    // are real branches to a label — so each edge's copies
                    // can be inlined directly along that edge's own path,
                    // with no intermediate copy-block indirection needed.
                    match (t_copies, f_copies) {
                        // Plain branch, no phi consumers on either edge —
                        // exactly Task 12's original (correct) shape.
                        (None, None) => {
                            g.emit(format!("    BZ {lf}"));
                            g.emit(format!("    BRA {lt}"));
                        }
                        // Only the f (cond==0) edge feeds a phi: BZ can't
                        // jump straight to `lf` anymore (the copies must
                        // run first), so it falls through into the copies
                        // instead; the t edge, needing none, still gets a
                        // direct branch.
                        (None, Some(cf)) => {
                            g.emit(format!("    BNZ {lt}"));
                            emit_phi_copies(&mut g, &cf, doms[&b.label].contains(&bc.f));
                            g.emit(format!("    BRA {lf}"));
                        }
                        // Only the t (cond!=0) edge feeds a phi: BZ still
                        // jumps straight to `lf` (no copies needed there);
                        // falling through (cond!=0) runs t's copies first.
                        (Some(ct), None) => {
                            g.emit(format!("    BZ {lf}"));
                            emit_phi_copies(&mut g, &ct, doms[&b.label].contains(&bc.t));
                            g.emit(format!("    BRA {lt}"));
                        }
                        // Both edges feed a phi: BZ routes to a fresh local
                        // label that runs the f-edge's copies, so neither
                        // edge's copies ever run on the other edge's path.
                        (Some(ct), Some(cf)) => {
                            let l_fcopies = g.fresh_label();
                            g.emit(format!("    BZ {l_fcopies}"));
                            emit_phi_copies(&mut g, &ct, doms[&b.label].contains(&bc.t));
                            g.emit(format!("    BRA {lt}"));
                            g.emit_label(&l_fcopies);
                            emit_phi_copies(&mut g, &cf, doms[&b.label].contains(&bc.f));
                            g.emit(format!("    BRA {lf}"));
                        }
                    }
                }
                Some(Inst::Ret(None)) => g.emit("    RETURN".to_string()),
                Some(Inst::Ret(Some((ty, v)))) => {
                    for i in 0..ty.bytes() {
                        g.emit_load_w(v, i);
                        let (a, f2) = g.operand(g.retval_lo + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        g.emit(format!("    MOVWF 0x{f2:03X},{bank}"));
                    }
                    g.emit("    RETURN".to_string());
                }
                _ => panic!("isel-pic18: block has no terminator"),
            }
        }
        out.extend(g.out);
    }
    // `__start` calls `main` and halts; matches the shape `isel::select`
    // uses for its own program entry, minus the ISR machinery (P5).
    out.push("__start:".to_string());
    out.push("    call main".to_string());
    out.push("    sleep".to_string());
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the `tmp` field's shared-borrow requirement:
    /// `Gen::tmp` must be `&'m mut u32` backed by one counter that outlives
    /// every function's `Gen`, not an owned `u32` reset per function — an
    /// owned counter would let two functions' `fresh_label()` calls both
    /// emit `tmp0`, a duplicate label that fails to assemble once Task 12
    /// starts calling `fresh_label()` for real. `fresh_label()` itself has
    /// no caller yet in Task 3's scope (Select/Icmp/Br land later), so this
    /// constructs two `Gen`s directly — the way `select()` constructs one
    /// per function — sharing one backing `tmp: &mut u32`, exactly as
    /// `select()` does across its `for f in &m.funcs` loop.
    #[test]
    fn fresh_label_counter_is_shared_across_gens() {
        let m = ir::parse("fn f(void) ()\n  block entry:\n    ret void\n");
        let addrs: HashMap<String, u16> = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let l1 = {
            let mut g = Gen {
                m: &m,
                addrs: &addrs,
                resolved: &resolved,
                retval_lo: 0,
                bsr: None,
                cur_func: "f",
                tmp: &mut tmp,
                out: Vec::new(),
            };
            g.fresh_label()
        };
        let l2 = {
            let mut g = Gen {
                m: &m,
                addrs: &addrs,
                resolved: &resolved,
                retval_lo: 0,
                bsr: None,
                cur_func: "f",
                tmp: &mut tmp,
                out: Vec::new(),
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
    use ir::parse;

    fn gen<'a>(
        m: &'a Module,
        addrs: &'a HashMap<String, u16>,
        resolved: &'a HashMap<String, (Base, u8, Vec<(u8, String)>)>,
        tmp: &'a mut u32,
    ) -> Gen<'a> {
        Gen {
            m,
            addrs,
            resolved,
            retval_lo: 0,
            bsr: None,
            cur_func: "main",
            tmp,
            out: Vec::new(),
        }
    }

    #[test]
    fn low_access_bank_needs_no_movlb() {
        let m = Module { globals: Vec::new(), funcs: Vec::new() };
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        assert_eq!(g.operand(0x05F), (0, 0x5F));
        assert!(g.out.is_empty(), "no MOVLB for the low access-bank range");
    }

    #[test]
    fn banked_gpr_range_needs_movlb() {
        let m = Module { globals: Vec::new(), funcs: Vec::new() };
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        assert_eq!(g.operand(0x0090), (1, 0x90));
        assert!(g.out.iter().any(|l| l.contains("MOVLB")), "the banked range needs a MOVLB");
    }

    #[test]
    fn sfr_high_segment_needs_no_movlb() {
        let m = Module { globals: Vec::new(), funcs: Vec::new() };
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        // FSR0L, the address this task exists to fix.
        assert_eq!(g.operand(0xFE9), (0, 0xE9), "the SFR segment is access-bank, a=0");
        assert!(g.out.is_empty(), "no MOVLB for an SFR address, regardless of the tracked BSR");
    }

    #[test]
    fn object_span_reports_a_global_arrays_byte_size() {
        let m = parse("global g i32\nfn main(void) ()\n  block entry:\n    ret void\n");
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        assert_eq!(g.object_span(&Base::Global("g".to_string())), 4);
    }

    #[test]
    fn object_span_reports_an_allocas_declared_size() {
        let m = parse("fn main(void) ()\n  block entry:\n    %buf = alloca 6\n    ret void\n");
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        assert_eq!(g.object_span(&Base::Slot("buf".to_string(), false)), 6);
    }
}
