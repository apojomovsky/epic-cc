//! `isel-pic18`: instruction selection for the PIC18 integer spine (P2),
//! extended by P3 (pointers, arrays, structs) and P4 (`const` in flash via
//! `TBLRD`). See docs/29-pic18-port-design.md §4 for phasing, and
//! docs/adr/ADR-009-pic18-pointer-model.md (P3) and
//! docs/adr/ADR-010-pic18-const-tblrd.md (P4: `const` globals read via
//! `TBLRD`, the 511-byte `RETLW` ceiling of PIC14 is gone, stores through
//! a `const` base panic loudly).
//! A separate crate from `isel` per docs/29-pic18-port-design.md §2 D-1:
//! the instruction sets differ enough that sharing would leak an
//! abstraction, and PIC14's working code must never be at risk from a
//! PIC18 edit.

use std::collections::{HashMap, HashSet};

use device::Device;
use ir::{Func, Inst, Module, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};

/// The high Access Bank segment's start: every classic-mode PIC18's SFRs
/// live at `0xF60-0xFFF` (160 bytes) by the core's own linear-addressing
/// definition (gputils' `.lkr` scripts declare it as a second, fixed
/// `ACCESSBANK` region, `accesssfr`, on both devices this core ships).
/// Unlike the low segment's `0x00-0x5F` (`Device::access_bank`, real
/// per-device data), the schema has no field for this because nothing has
/// needed one yet: it is architecture, not silicon (epic-cc#226 audit).
const PIC18_SFR_ACCESS_LO: u16 = 0xF60;

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
    /// docs/adr/ADR-009-pic18-pointer-model.md.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    /// Fixed, `BSR`-independent return-value region (up to 4 bytes, from
    /// `device.fixed_retval`)  -  see the plan's "Where the retval/scratch
    /// design comes from" section.
    retval_lo: u16,
    /// The access bank's high bound (from `device.access_bank`), soft-float
    /// runtime routines' frame-fits-in-the-access-bank assertions bound
    /// against. Every classic PIC18 device has shared this exact value
    /// (`0x5F`) so far (epic-cc#226 audit), but it is device data, not an
    /// architectural constant guaranteed for every future PIC18 part.
    access_bank_hi: u16,
    /// The `BSR` value the last-emitted `MOVLB` set, or `None` when it's
    /// unknown (module start, or just after a label  -  branch targets can
    /// be reached with any prior `BSR` state, so it must be re-established
    /// on the next banked access rather than assumed).
    bsr: Option<u8>,
    cur_func: &'m str,
    /// True when this function is an interrupt handler: the function body
    /// runs with a save prologue / restore epilogue and `RETFIE` instead of
    /// a plain `RETURN` (P5, single-vector compatibility mode).
    isr: bool,
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
    /// Every label in this file  -  a real block label, a fresh
    /// `Select`/`Icmp` branch target, or a synthesized phi-copy label
    /// is a place code from more than one preceding path can land, and
    /// each of those paths may have executed a different subset of the
    /// `MOVLB`s that led here (or none at all). `operand()`'s `MOVLB`
    /// elision is only sound when `self.bsr` reflects what's ACTUALLY
    /// true on every path reaching the current point, so any label must
    /// reset it  -  trusting a stale tracked value across a branch target
    /// has been the exact root cause of three separate miscompile bugs
    /// found across this task's review rounds (`Select`'s `l_else` and
    /// `l_end`, `BrCond`'s synthesized `l_fcopies`, and  -  narrower, but
    /// the same class  -  `emit_icmp_i16`'s shared `l_true`/`l_false`).
    /// This helper makes the reset structural instead of a fact every
    /// label call site has to individually remember: every
    /// `self.emit(format!("{{...}}:"))`/`g.emit(format!("{{...}}:"))` in
    /// this file routes through this instead.
    ///
    /// Labels are not the ONLY BSR-clobbering join point, though: a
    /// `CALL` return is another one (the callee runs its own arbitrary
    /// `MOVLB`s and never restores the caller's bank on `RETURN`), but it
    /// is not itself a label, so it structurally cannot go through this
    /// helper  -  `Inst::Call`'s arm in `emit_inst` resets `self.bsr`
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

    /// Whether `name` is a `const` (flash) global: read via `TBLRD`, never
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
    /// Whether `name` is a function (a valid indirect-call target) rather
    /// than a RAM/const global. A function's address is a link-time label
    /// literal, materialized as LOW/HIGH bytes, never looked up in the
    /// address map (epic-cc#73).
    fn is_function(&self, name: &str) -> bool {
        self.m.funcs.iter().any(|f| f.name == name)
    }

    /// The byte width of a value-defining register in the current function
    /// (its slot is `bytes` wide), for scaling a dynamic const-table index
    /// when the index register is 16-bit. Mirrors `alloc::def_width`'s
    /// width rules (an `icmp` result is i1 -> 1 byte) so the addition code
    /// knows whether to propagate into a high byte.
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
    /// instruction. An address past the 12-bit data space panics loudly.
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
    /// to fold, which `resolve_pointers` has no case for yet (P3 scope: see
    /// ADR-009). Panics loudly rather than silently emitting a bogus
    /// access.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved.get(&key).cloned().unwrap_or_else(|| {
            panic!(
                "isel-pic18: pointer %{r} ({key}) has no resolved base; only globals, \
                 allocas, byval/sret params, plain pointer params and gep chains off \
                 them resolve (see ADR-009, extended by ADR-018)"
            )
        })
    }

    /// Whether pointer-select dst `name` was seeded by iselcore as an
    /// indirect slot (`Base::Slot(_, true)`): its bytes are a runtime
    /// address VALUE the select must materialize, not a folded pointer.
    fn select_is_seeded(&self, name: &str) -> bool {
        matches!(
            self.resolved.get(&ssa_key(self.cur_func, name)),
            Some((Base::Slot(_, true), 0, t)) if t.is_empty()
        )
    }

    /// The `,A`/`,B` operand components `(a, f)` for a physical address
    /// used by a `W`-routing instruction (`ADDWF`/`SUBWF`/.../`CPFSxx`/
    /// `MOVWF`), emitting `MOVLB` first if the tracked `BSR` doesn't
    /// already match. PIC18's Access Bank is TWO disjoint ranges: the low
    /// general-purpose segment (`0x000-0x05F`) and the high SFR segment
    /// (`0xF60-0xFFF`, every SFR, including
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
        if addr <= self.access_bank_hi || addr >= PIC18_SFR_ACCESS_LO {
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

    /// One byte, memory-to-memory, via `MOVFF`  -  no access bit, no `BSR`.
    fn emit_copy_byte(&mut self, src: u16, dst: u16) {
        self.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
    }

    /// Copy the two-byte ADDRESS VALUE of `val` into the slot at `dst`:
    /// a `Const` literal writes the constant bytes, a `Global` writes its
    /// link-time address as two literals, a `Reg` copies the two bytes of
    /// its runtime-address slot (a seeded select dst, an IntToPtr dst, or
    /// a pointer param). Used by the pointer-select materialization
    /// (epic-cc#147); a reg with dynamic terms is a computed address with
    /// no single materializable value and panics.
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
                if self.is_function(g) {
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

    /// Copy `val` (width `ty.bytes()`) into the slot starting at `dst`. A
    /// register/global source uses `MOVFF` (no access bit needed); a
    /// constant has no `MOVFF` literal form, so it goes through `W` via
    /// `MOVLW`/`MOVWF` (which DOES need the access bit  -  this is the one
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
                // (epic-cc#193) can propagate a global argument into a
                // helper's pointer parameter, so a GEP over it resolves
                // here to `Base::Global`, not a slot. Its base bytes are
                // literals (`MOVLW`), not a `MOVF` read, so it needs its
                // own arm, mirroring `emit_move_addr_to_slot`'s above plus
                // the slot case's k/terms folding.
                if let iselcore::Base::Global(name) = &base {
                    assert!(
                        k == 0 || terms.is_empty(),
                        "isel-pic18: GEP with both k and terms not supported in move"
                    );
                    let addr = self.global_addr(name).wrapping_add(k as u16);
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
                            _ => panic!(
                                "isel-pic18: multi-term GEP move with {terms:?} not supported"
                            ),
                        }
                        let (a, f) = self.operand(dst + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                    }
                    return;
                }
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
                // = LOW(g), byte 1 = HIGH(g) (epic-cc#73).
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
                // literals, never copy the pointee's contents (epic-cc#155).
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
                            Addr::Direct(
                                self.global_addr(name) + u16::from(k) + u16::from(byte_off),
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
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
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
                            Some(self.global_addr(name) + u16::from(k) + u16::from(byte_off))
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
                            Some(sa + u16::from(k) + u16::from(byte_off))
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
    fn emit_fsr1_dynamic(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P3 scope; {} terms)",
            terms.len()
        );
        let static_part = u16::from(k) + u16::from(byte_off);
        let lit = (base_addr + static_part) & 0xFFF;
        self.emit(format!("    LFSR 1, 0x{lit:03X}"));
        self.add_term_to_fsr1(terms);
    }
    /// `slot_addr` holds a 2-byte ADDRESS (an sret param's contents), not
    /// the object itself: load THAT address into FSR1, then add the static
    /// offset and any dynamic term. FSR1 mirror of `emit_fsr0_indirect_slot`.
    fn emit_fsr1_indirect_slot(
        &mut self,
        slot_addr: u16,
        k: u8,
        terms: &[(u8, String)],
        byte_off: u8,
    ) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P3 scope; {} terms)",
            terms.len()
        );
        self.emit_copy_byte(slot_addr, 0xFE1); // FSR1L = low byte of the stored address
        self.emit_copy_byte(slot_addr + 1, 0xFE2); // FSR1H = high byte
        let static_part = u16::from(k) + u16::from(byte_off);
        if static_part != 0 {
            self.emit(format!("    MOVLW 0x{:02X}", static_part & 0xFF));
            let (fa, ff) = self.operand(0xFE1);
            self.emit(format!(
                "    ADDWF 0x{ff:03X},F,{}",
                if fa == 0 { "A" } else { "B" }
            ));
            self.emit(format!("    MOVLW 0x{:02X}", static_part >> 8));
            let (ha, hf) = self.operand(0xFE2);
            self.emit(format!(
                "    ADDWFC 0x{hf:03X},F,{}",
                if ha == 0 { "A" } else { "B" }
            ));
        }
        self.add_term_to_fsr1(terms);
    }
    /// Add the single dynamic term (if any) onto `FSR1L`/`FSR1H` with
    /// carry, `scale` times. FSR1 mirror of `add_term_to_fsr0`.
    fn add_term_to_fsr1(&mut self, terms: &[(u8, String)]) {
        if let Some((scale, reg)) = terms.first() {
            let a = self.slot_addr(self.cur_func, reg).direct();
            for _ in 0..*scale {
                let (ra, rf) = self.operand(a);
                self.emit(format!(
                    "    MOVF 0x{rf:03X},W,{}",
                    if ra == 0 { "A" } else { "B" }
                ));
                let (fa, ff) = self.operand(0xFE1); // FSR1L
                self.emit(format!(
                    "    ADDWF 0x{ff:03X},F,{}",
                    if fa == 0 { "A" } else { "B" }
                ));
                self.emit("    MOVLW 0x00".to_string());
                let (ha, hf) = self.operand(0xFE2); // FSR1H
                self.emit(format!(
                    "    ADDWFC 0x{hf:03X},F,{}",
                    if ha == 0 { "A" } else { "B" }
                ));
            }
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
        self.add_term_to_fsr0(terms);
    }
    /// `slot_addr` holds a 2-byte ADDRESS (an sret param's contents), not
    /// the object itself: load THAT address into FSR0 (`MOVFF slot,
    /// FSR0L` / `MOVFF slot+1, FSR0H`, both plain memory-to-memory, no
    /// access bit needed since MOVFF never uses one), then add the static
    /// offset (`k + byte_off`) and any dynamic term the same way
    /// `emit_fsr0_dynamic` does, and access through `INDF0`.
    fn emit_fsr0_indirect_slot(
        &mut self,
        slot_addr: u16,
        k: u8,
        terms: &[(u8, String)],
        byte_off: u8,
    ) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P3 scope; {} terms)",
            terms.len()
        );
        self.emit_copy_byte(slot_addr, 0xFE9); // FSR0L = low byte of the stored address
        self.emit_copy_byte(slot_addr + 1, 0xFEA); // FSR0H = high byte
        let static_part = u16::from(k) + u16::from(byte_off);
        if static_part != 0 {
            self.emit(format!("    MOVLW 0x{:02X}", static_part & 0xFF));
            let (fa, ff) = self.operand(0xFE9);
            self.emit(format!(
                "    ADDWF 0x{ff:03X},F,{}",
                if fa == 0 { "A" } else { "B" }
            ));
            self.emit(format!("    MOVLW 0x{:02X}", static_part >> 8));
            let (ha, hf) = self.operand(0xFEA);
            self.emit(format!(
                "    ADDWFC 0x{hf:03X},F,{}",
                if ha == 0 { "A" } else { "B" }
            ));
        }
        self.add_term_to_fsr0(terms);
    }
    /// Add the single dynamic term (if any) onto `FSR0L`/`FSR0H` with
    /// carry, `scale` times: `MOVF %reg,W; ADDWF FSR0L,F; MOVLW 0;
    /// ADDWFC FSR0H,F`. Shared by `emit_fsr0_dynamic` and
    /// `emit_fsr0_indirect_slot`, the only two FSR0 setups that carry a
    /// runtime term.
    fn add_term_to_fsr0(&mut self, terms: &[(u8, String)]) {
        if let Some((scale, reg)) = terms.first() {
            let a = self.slot_addr(self.cur_func, reg).direct();
            for _ in 0..*scale {
                let (ra, rf) = self.operand(a);
                self.emit(format!(
                    "    MOVF 0x{rf:03X},W,{}",
                    if ra == 0 { "A" } else { "B" }
                ));
                let (fa, ff) = self.operand(0xFE9); // FSR0L
                self.emit(format!(
                    "    ADDWF 0x{ff:03X},F,{}",
                    if fa == 0 { "A" } else { "B" }
                ));
                self.emit("    MOVLW 0x00".to_string());
                let (ha, hf) = self.operand(0xFEA); // FSR0H
                self.emit(format!(
                    "    ADDWFC 0x{hf:03X},F,{}",
                    if ha == 0 { "A" } else { "B" }
                ));
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
    fn const_base_of(&self, ptr: &Val) -> Option<(String, u8, Vec<(u8, String)>)> {
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
    fn emit_tblptr_static(&mut self, table: &str, k: u8, byte_off: u8) {
        for (lit, reg) in [
            (format!("LOW({table})"), 0xF6),
            (format!("HIGH({table})"), 0xF7),
            (format!("UPPER({table})"), 0xF8),
        ] {
            self.emit(format!("    MOVLW {lit}"));
            self.emit(format!("    MOVWF 0x{reg:02X},A"));
        }
        let static_part = u16::from(k) + u16::from(byte_off);
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
    /// `scale` times: `MOVF %reg_lo,W; ADDWF TBLPTRL,F; MOVLW 0;
    /// ADDWFC TBLPTRH,F; ADDWFC TBLPTRU,F`, plus (for a 16-bit index
    /// register) the high byte added onto `TBLPTRH` with its own carry.
    /// `MOVLW` never touches C, so the ADDWF-set carry survives into the
    /// `ADDWFC`s, the same discipline P3's `add_term_to_fsr0` relies on.
    /// A 16-bit index needs its high byte folded in or `table[0x1XX]`
    /// reads the wrong byte, which is why `reg_width` is consulted.
    fn add_dynamic_to_tblptr(&mut self, terms: &[(u8, String)]) {
        if let Some((scale, reg)) = terms.first() {
            let lo = self.slot_addr(self.cur_func, reg).direct();
            let width = self.reg_width(reg);
            for _ in 0..*scale {
                let (ra, rf) = self.operand(lo);
                self.emit(format!(
                    "    MOVF 0x{rf:03X},W,{}",
                    if ra == 0 { "A" } else { "B" }
                ));
                self.emit("    ADDWF 0xF6,F,A".to_string()); // TBLPTRL += idx_lo
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    ADDWFC 0xF7,F,A".to_string());
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    ADDWFC 0xF8,F,A".to_string());
                if width == 2 {
                    let (ha, hf) = self.operand(lo + 1);
                    self.emit(format!(
                        "    MOVF 0x{hf:03X},W,{}",
                        if ha == 0 { "A" } else { "B" }
                    ));
                    self.emit("    ADDWF 0xF7,F,A".to_string());
                    self.emit("    MOVLW 0x00".to_string());
                    self.emit("    ADDWFC 0xF8,F,A".to_string());
                }
            }
        }
    }

    /// One `const` (flash) byte read: `TBLPTR = table_base + k + Σ terms +
    /// byte_off`, `TBLRD*` (no auto-increment: per-byte re-setup keeps
    /// every read independent, mirroring P3's per-byte FSR0 re-setup), then
    /// `MOVFF TABLAT, dst`. Multi-byte loads call this once per byte with
    /// an increasing `byte_off`.
    fn emit_const_load_byte(
        &mut self,
        table: &str,
        k: u8,
        terms: &[(u8, String)],
        byte_off: u8,
        dst: u16,
    ) {
        assert!(
            terms.len() <= 1,
            "isel-pic18: multi-term dynamic pointer offsets not yet supported (P4 scope; {} terms)",
            terms.len()
        );
        self.emit_tblptr_static(table, k, byte_off);
        self.add_dynamic_to_tblptr(terms);
        self.emit("    TBLRD*".to_string());
        self.emit_copy_byte(0xFF5, dst); // TABLAT -> dst
    }

    /// Copy each call arg into the callee's `{func}::{param}` slots. Shared
    /// by the direct call path and the per-candidate arms of an indirect
    /// call chain (epic-cc#73).
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
                // region at the running offset (epic-cc#131).
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
                // `sret` means "store the 2-byte ADDRESS `arg.val`
                // points to into the callee's sret slot"  -  same
                // const hazard as `byval` above: an sret arg is
                // always meant to be a pointer, so a literal here
                // has no sensible meaning. `emit_ptr_setup` resolves
                // `arg.val` the same way every other pointer
                // consumer does: `Addr::Direct` is a compile-time
                // address (write its two bytes as literals);
                // `Addr::Indirect` means FSR0 now HOLDS the
                // resolved runtime address (a dynamic target, or a
                // sret-of-sret target), so the slot gets FSR0's two
                // bytes via MOVFF instead of a literal.
                assert!(
                    !matches!(arg.val, Val::Const(_)),
                    "isel-pic18: const sret call arg not yet supported"
                );
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
                        // byte 0 = LOW(g), byte 1 = HIGH(g) (epic-cc#73). A
                        // param-forwarded callback (epic-cc#137) arrives as
                        // such an arg.
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
                    // address at runtime (epic-cc#155).
                    if !self.resolved.contains_key(&ssa_key(self.cur_func, r)) {
                        let sa = self.slot_addr(self.cur_func, r).direct();
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
                // M15 conversion ABI: __uitofp_f32/__sitofp_f32 take a 4-byte val slot
                // but i8/i16 sources copy only their width; stale high bytes corrupt
                // the leading-1 search (P14 fix: isel/src/lib.rs:1618-1661). Fill them.
                if (ty.bytes() as u16) < u16::from(callee.params[i].width) {
                    assert_eq!(
                        callee.params[i].width, 4,
                        "isel-pic18: narrow scalar arg {} of @{} into non-4-byte param",
                        i, func
                    );
                    let aw = ty.bytes() as u16;
                    match func {
                        "__uitofp_f32" => {
                            for j in aw..4 {
                                self.emit(format!("    CLRF 0x{:03X},A", pa + j));
                            }
                        }
                        "__sitofp_f32" => {
                            let sign = pa + aw - 1;
                            if aw == 2 {
                                self.emit(format!("    MOVF 0x{sign:03X},W,A"));
                                self.emit(format!("    MOVWF 0x{:03X},A", pa + 2));
                                self.emit(format!("    MOVWF 0x{:03X},A", pa + 3));
                            } else {
                                assert_eq!(
                                    aw, 1,
                                    "isel-pic18: unexpected narrow width for @__sitofp_f32"
                                );
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit(format!("    BTFSC 0x{sign:03X},7,A"));
                                self.emit("    MOVLW 0xFF".to_string());
                                for j in 1..4 {
                                    self.emit(format!("    MOVWF 0x{:03X},A", pa + j));
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
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel-pic18: only i8/i16 loads supported");
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
                // P4: a load through a `const` (flash) base reads via
                // TBLRD, one independent read per byte. Everything else
                // (RAM globals, allocas, sret slots, dynamic pointers)
                // keeps the P2/P3 FSR/INDF path.
                if let Some((table, k, terms)) = self.const_base_of(&ptr_val) {
                    for kk in 0..l.ty.bytes() {
                        self.emit_const_load_byte(&table, k, &terms, kk, dst + u16::from(kk));
                    }
                    return;
                }
                for k in 0..l.ty.bytes() {
                    match self.emit_ptr_setup(&ptr_val, k) {
                        Addr::Direct(src) => self.emit_copy_byte(src, dst + u16::from(k)),
                        Addr::Indirect => {
                            self.emit(format!("    MOVFF 0xFEF, 0x{:03X}", dst + u16::from(k)))
                        }
                    }
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel-pic18: only i8/i16 stores supported");
                // Literal-pointer (SFR) store: `inttoptr` form, a direct
                // physical address. A register/global source copies via
                // MOVFF (no access bit); a constant goes through W with
                // `operand()`'s access-bit (a=0 for the SFR segment, no
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
                // P4: a store through a `const` (flash) base is a write to
                // ROM. It must panic loudly (matching PIC14's
                // store-through-const panic), never silently emit a
                // MOVFF/MOVWF that the simulator would apply to a RAM
                // alias of the same low address.
                if self.const_base_of(&ptr_val).is_some() {
                    panic!(
                        "isel-pic18: ROM is not writable: store through const global {ptr_val:?}"
                    );
                }
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
                            self.emit_load_w(&s.val, k);
                            self.emit("    MOVWF 0xFEF,A".to_string()); // INDF0
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
                // Milestone-8 shifts (P6): a const count inlines as a fixed
                // RLCF/RRCF sequence; k == 0 is a plain copy; k >= width is
                // LLVM poison and panics loudly. A variable (reg) count must
                // never reach isel: legalize rewrites it to a routine call.
                // Without this arm a shift would hit the `(other, _)`
                // panic below.
                let av = self.val_addr(&b.a).direct();
                let dst = self.slot_addr(self.cur_func, &b.dst).direct();
                // Milestone-8 shifts (P6): a const count inlines as a fixed
                // RLCF/RRCF sequence; k == 0 is a plain copy; k >= width is
                // LLVM poison and panics loudly. A variable (reg) count must
                // never reach isel: legalize rewrites it to a routine call.
                // Without this arm a shift would hit the `(other, _)`
                // panic below.
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
                    // Copy the value into the dst slot, then rotate the dst
                    // in place k times. shl: lo then hi (carry goes up);
                    // lshr: hi then lo (bits come down); ashr: set C from
                    // the sign bit before each rrcf so the sign fills every
                    // vacated bit.
                    self.emit_move_val_to_slot(&b.a, b.ty, dst);
                    for _ in 0..k {
                        match b.op {
                            ir::BinOp::Shl => {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in 0..n {
                                    let (da, df) = self.operand(dst + u16::from(i));
                                    let dbank = if da == 0 { "A" } else { "B" };
                                    self.emit(format!("    RLCF 0x{df:03X},F,{dbank}"));
                                }
                            }
                            ir::BinOp::LShr => {
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in (0..n).rev() {
                                    let (da, df) = self.operand(dst + u16::from(i));
                                    let dbank = if da == 0 { "A" } else { "B" };
                                    self.emit(format!("    RRCF 0x{df:03X},F,{dbank}"));
                                }
                            }
                            ir::BinOp::AShr => {
                                let hi = dst + u16::from(n - 1);
                                let (ha, hf) = self.operand(hi);
                                let hbank = if ha == 0 { "A" } else { "B" };
                                self.emit(format!("    BTFSC 0x{hf:03X},7,{hbank}"));
                                self.emit("    BSF 0xFD8,0,A".to_string()); // STATUS C
                                self.emit(format!("    BTFSS 0x{hf:03X},7,{hbank}"));
                                self.emit("    BCF 0xFD8,0,A".to_string()); // STATUS C
                                for i in (0..n).rev() {
                                    let (da, df) = self.operand(dst + u16::from(i));
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
                            };
                            // Re-enter the Bin arm with swapped operands via
                            // the normal per-byte loop: emit the swapped bin
                            // directly here to avoid recursion.
                            let av = self.val_addr(&swapped.a).direct();
                            for i in 0..n {
                                self.emit_load_w(&swapped.b, i);
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
                                let (dacc, df) = self.operand(dst + u16::from(i));
                                let dbank = if dacc == 0 { "A" } else { "B" };
                                self.emit(format!("    MOVWF 0x{df:03X},{dbank}"));
                            }
                            return;
                        }
                        ir::BinOp::Sub => {
                            // `k - a` for `n > 1`: byte 0 via `SUBLW`
                            // (k - W), bytes 1..n via `k_i + ~a_i + C0`
                            // (the borrow-aware `k - a - !C0` chain). `C0`
                            // is saved in a flag bit (0x0000,0 in the
                            // reserved `fixed_retval`/`retval_lo` region, so
                            // it is live-free across a `Bin` and an
                            // interrupt mid-sequence cannot clobber a live
                            // local) before the `COMF`/`ADDLW` overwrites
                            // STATUS.
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
                                self.emit(format!("    ADDLW 0x{kb:02X}"));
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
                        (other, _) => {
                            panic!("isel-pic18: Bin op {other:?} not yet implemented (Task 6+)")
                        }
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
                // `val_addr` maps `Val::Const(k)` to a RAM ADDRESS
                // (`k & 0xFF`), not a literal  -  same hazard already guarded
                // for `Bin`/`Icmp` above. Not known to be reachable from
                // clang-generated IR or the differential fuzzer (both only
                // ever cast a loaded/computed register, never a bare
                // literal  -  a literal cast is constant-foldable by the
                // frontend before it ever reaches this backend), but the
                // cheap guard costs nothing and keeps a future const-source
                // producer from silently miscompiling instead of panicking.
                assert!(
                    !matches!(z.val, Val::Const(_)),
                    "isel-pic18: const source Zext not yet supported"
                );
                // Mirrors `isel`'s own width guard (`crates/isel/src/lib.rs`,
                // "isel: zext must not narrow")  -  without it, a malformed
                // `Zext` with `to.bytes() < from.bytes()` would copy
                // `from.bytes()` bytes into a narrower `to.bytes()` slot
                // below, writing past the destination into whatever local
                // sits next to it. Equal widths (e.g. `zext i1 to i8`,
                // where both types report `.bytes() == 1` in the byte
                // model  -  an icmp result is materialized as a byte holding
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
                // Same width-relationship hazard as `Inst::Zext` above,
                // mirroring `isel`'s own sext bounds guard
                // (`crates/isel/src/lib.rs`, "isel: sext only supports
                // i8/i16 -> i16/i32"): without it, `to.bytes() <=
                // from.bytes()` would sign-fill zero or a negative number
                // of high bytes and still have copied `from.bytes()` bytes
                // into a same-or-narrower `to.bytes()` slot, writing past
                // the destination.
                // `i1 -> iN` is a 0/1 value: an i1's stored byte is exactly
                // 0 or 1 (every i1 producer clears the high bits), so the
                // "sign" fill is zero and a plain copy IS the sext. Panic
                // only on equal-or-narrowing widths that would write past
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
                if s.from == Ty::I1 && s.to.bytes() > s.from.bytes() {
                    for i in s.from.bytes()..s.to.bytes() {
                        self.emit("    MOVLW 0x00".to_string());
                        let (a, f) = self.operand(dst + u16::from(i));
                        let bank = if a == 0 { "A" } else { "B" };
                        self.emit(format!("    MOVWF 0x{f:03X},{bank}"));
                    }
                    // the i1 widening already zero-filled: skip the
                    // sign-fill loop below
                    return;
                }
                // The sign-fill byte(s) must reflect the SOURCE's actual
                // sign bit (bit 7 of its highest byte) at the time this
                // cast runs  -  not an assumption. `MOVLW 0x00` first, then
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
                    // runtime slot, two runtime slots, epic-cc#147): iselcore
                    // seeded the dst as an indirect slot, so the selected
                    // arm's address bytes must land in it. The two-byte
                    // value select below materializes them.
                } else if s.ptr {
                    // A pointer-typed select folded by iselcore into the
                    // resolved map (a GEP-chain select): emits nothing, every
                    // load/store through it lowers via the fold. PIC18 has
                    // no const-arm fold today, so this is the GEP-arm shape.
                    assert!(
                        !matches!(s.cond, Val::Const(_)),
                        "isel-pic18: const cond pointer Select not yet supported"
                    );
                }
                // `a`/`b` route through `emit_move_val_to_slot`, which
                // handles `Val::Const` correctly (via `MOVLW`+`MOVWF`, not
                // as a RAM address through `val_addr`) and never branches
                // on a flag  -  no guard needed for either. A seeded
                // pointer select (epic-cc#147) holds an ADDRESS VALUE, so
                // its arms go through `emit_move_addr_to_slot` instead.
                //
                // `cond` is different: it's loaded via `emit_load_w`, then
                // immediately tested with `BZ`, which relies on the LOAD
                // having set the Z flag from `cond`'s value. That's true
                // when `cond` is `Val::Reg`/`Val::Global` (`emit_load_w`
                // emits `MOVF ...,W`, and this project's simulator's MOVF
                // calls `set_zn`  -  `crates/sim/src/lib.rs:779-783`). It is
                // NOT true for `Val::Const`: `emit_load_w`'s const arm
                // emits only `MOVLW`, and the simulator's MOVLW (PIC18
                // opcode 0xE, `crates/sim/src/lib.rs:903`) does `self.w = k`
                // with no `set_zn` call at all  -  so `BZ` would test
                // whatever Z flag the PREVIOUS instruction happened to
                // leave, silently picking the wrong side of the `Select`.
                // Same hazard class as the const-LHS/const-source guards
                // elsewhere in this file; guard it the same way.
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
                    self.emit_load_w(&s.cond, 0);
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
                    // ISR-visible global, epic-cc#137). Emit the
                    // deterministic trap loop rather than panic on the
                    // register name or silently call nothing.
                    let l_trap = self.fresh_label();
                    self.emit_label(&l_trap);
                    self.emit(format!("    BRA {l_trap}"));
                } else {
                    self.emit_call_args(&c.func, &c.args);
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
            }

            Inst::Alloca(_) | Inst::Gep(_) => {
                // Virtual: Alloca's slot comes from `alloc`'s layout and
                // Gep's result is folded away by `resolve_pointers`
                // before codegen ever runs; see this file's module doc and
                // docs/adr/ADR-009-pic18-pointer-model.md. Neither emits
                // anything of its own.
            }
            Inst::Memcpy(mc) => match &mc.len {
                ir::MemLen::Const(n) => {
                    for i in 0..*n {
                        // The source pointer is set up on FSR1 (an indirect
                        // source would otherwise be clobbered by the
                        // destination's FSR0 setup), the destination on
                        // FSR0, and the byte moves through W. W is
                        // ISR-saved (0x0004), so an interrupt taken
                        // mid-copy cannot corrupt the held byte.
                        // A `const` (flash) source has no RAM address: the
                        // byte is read via TBLRD into TABLAT, then moved
                        // from there (epic-cc#143: default-struct initializer
                        // copies from `@__const.*`).
                        if let Some((table, k, terms)) = self.const_base_of(&mc.src) {
                            // Seed TBLPTR at the flash source byte, read it
                            // into TABLAT, then move TABLAT (0xFF5) to the
                            // destination (direct or FSR0-indirect).
                            self.emit_tblptr_static(&table, k, i as u8);
                            self.add_dynamic_to_tblptr(&terms);
                            self.emit("    TBLRD*".to_string());
                            match self.emit_ptr_setup(&mc.dst, i) {
                                Addr::Direct(dst) => {
                                    self.emit(format!("    MOVFF 0xFF5, 0x{dst:03X}"));
                                }
                                Addr::Indirect => {
                                    self.emit("    MOVFF 0xFF5, 0xFEF".to_string());
                                }
                            }
                            continue;
                        }
                        let src_direct = self.emit_memcpy_src_setup(&mc.src, i);
                        match self.emit_ptr_setup(&mc.dst, i) {
                            Addr::Direct(dst) => {
                                match src_direct {
                                    Some(a) => {
                                        self.emit(format!("    MOVFF 0x{a:03X}, 0x{dst:03X}"))
                                    }
                                    None => {
                                        self.emit("    MOVF 0xFE7,W,A".to_string()); // INDF1
                                        self.emit(format!("    MOVWF 0x{dst:03X},A"));
                                    }
                                }
                            }
                            Addr::Indirect => match src_direct {
                                Some(a) => {
                                    self.emit(format!("    MOVFF 0x{a:03X}, 0xFEF"));
                                }
                                None => {
                                    self.emit("    MOVFF 0xFE7, 0xFEF".to_string());
                                }
                            },
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
    /// deterministic trap loop rather than a silent wrong call (epic-cc#73).
    fn emit_indirect_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        let l_done = self.fresh_label();
        for cand in callees.iter() {
            let l_next = self.fresh_label();
            // Compare the fp value's two bytes against the candidate's
            // address. MOVF sets Z; XORLW leaves it; BNZ skips on mismatch.
            self.emit_load_w(&Val::Reg(func.to_string()), 0);
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit(format!("    BNZ {l_next}"));
            self.emit_load_w(&Val::Reg(func.to_string()), 1);
            self.emit(format!("    XORLW HIGH({cand})"));
            self.emit(format!("    BNZ {l_next}"));
            // Matched: copy args into this candidate's slots and call it.
            self.emit_call_args(cand, args);
            self.emit(format!("    CALL {cand}"));
            self.bsr = None;
            self.emit(format!("    BRA {l_done}"));
            self.emit_label(&l_next);
        }
        // No candidate matched: deterministic trap.
        let l_trap = self.fresh_label();
        self.emit_label(&l_trap);
        self.emit(format!("    BRA {l_trap}"));
        self.emit_label(&l_done);
        if let Some(d) = dst {
            let t = ty.expect("isel-pic18: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                self.emit_copy_byte(self.retval_lo + u16::from(i), da + u16::from(i));
            }
        }
    }

    /// `dst = (a <pred> b) ? 1 : 0` for one byte, via `a - b` (SUBWF: f=a,
    /// W=b beforehand, d=W so `a`'s slot is untouched) and a flag-based
    /// branch. C/Z/N/OV follow PIC18's standard (ARM-style) condition-code
    /// semantics  -  C=1 means "no borrow" (a>=b unsigned)  -  already relied
    /// on by P1's `Pic18::sub_flags`.
    ///
    /// Delegates the actual flag test to `emit_cmp_branch` (shared with
    /// the i16 path, Task 9). A single byte has no "next byte" to defer
    /// to, so the "equal" outcome must resolve directly to this
    /// predicate's real answer at equality: true for the non-strict/eq
    /// predicates (`eq`, `uge`, `ule`, `sge`, `sle`), false for the
    /// strict ones (`ne`, `ult`, `ugt`, `slt`, `sgt`)  -  NOT uniformly
    /// `l_false`, which would silently invert `uge`/`ule`/`sge`/`sle` at
    /// equality (e.g. `ule(5, 5)` must stay `1`).
    fn emit_icmp_byte(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // `val_addr` maps `Val::Const(k)` to a RAM ADDRESS (`k & 0xFF`),
        // not a literal  -  a constant on the LHS (e.g. `icmp ult i8 5, %x`)
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
    /// (offset 1) first, with `pred`'s own signedness: if it differs,
    /// that alone decides the whole 16-bit result, since the sign only
    /// ever lives in the most-significant byte. Only when the high bytes
    /// are equal does the low byte (offset 0) get compared, always
    /// **unsigned** (`slt`->`ult`, `sle`->`ule`, `sgt`->`ugt`, `sge`->`uge`;
    /// `ult`/`ule`/`ugt`/`uge` already are their own tie-break).
    ///
    /// `eq`/`ne` don't fit this "high byte decides" shape at all  -  they
    /// need BOTH bytes equal (`eq`) or EITHER byte different (`ne`), so
    /// they're dispatched to their own short-circuit, `emit_icmp_i16_eq_ne`,
    /// instead of the tie-break machinery built for the eight ordering
    /// predicates.
    fn emit_icmp_i16(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        // Same const-LHS hazard as `emit_icmp_byte`  -  this is a separate
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
        // 16-bit values are fully identical, so  -  unlike the high byte
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
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst);
    }

    /// `eq`/`ne` for multi-byte values: true (for `eq`) only when every
    /// byte matches; `ne` is the mirror. Direct per-byte equality checks
    /// (`SUBWF` + `BNZ`), independent of the signed/unsigned tie-break
    /// machinery used for the eight ordering predicates: a partial match
    /// (some byte equal, another different) is decisive here in a way it
    /// never is for `slt`/`ult`/etc.
    fn emit_icmp_i16_eq_ne(&mut self, a: Val, b: Val, pred: &str, dst: &str) {
        self.emit_icmp_eq_ne(a, b, pred, dst, 2);
    }

    fn emit_icmp_eq_ne(&mut self, a: Val, b: Val, pred: &str, dst: &str, bytes: u8) {
        let l_true = self.fresh_label();
        let l_false = self.fresh_label();
        let l_done = self.fresh_label();
        let l_mismatch = if pred == "eq" { &l_false } else { &l_true };
        for offset in 0..bytes {
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

    /// `dst = (a <pred> b) ? 1 : 0` for four bytes: compare the high byte
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
            self.emit_icmp_eq_ne(a, b, pred, dst, 4);
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
        self.emit_materialize_bool(&l_true, &l_false, &l_done, dst);
    }

    /// The shared flag-test core behind `emit_icmp_byte`/`emit_icmp_i16`:
    /// computes `a - b` (SUBWF, `W = a - b`) for the byte at `byte_offset`
    /// and branches on `pred`'s C/Z/N/OV condition  -  three ways, not two:
    /// `l_true` if `pred` holds for this byte pair, `l_false` if it
    /// definitely does not (the bytes differ in the "wrong" direction),
    /// or `l_equal` if the two bytes are equal. Equality is inherently
    /// ambiguous from a single byte's flags alone  -  the caller decides
    /// what it means: "go check the next byte" (i16's high-byte compare),
    /// or "that IS the final answer" (i8, and i16's low-byte tie-break),
    /// by choosing what `l_equal` points at.
    ///
    /// `eq`/`ne` are exempt from the three-way split  -  a byte's equality
    /// already IS their complete per-byte answer, so their arms use only
    /// `l_true`/`l_false`. i16's `Icmp` lowering never calls this for
    /// `eq`/`ne` (see `emit_icmp_i16_eq_ne`); i8's `emit_icmp_byte` does,
    /// with `l_equal` bound to whichever of `l_true`/`l_false` matches.
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
    /// `emit_icmp_i16_eq_ne`  -  the only difference between the three is
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

    /// Load byte `offset` of any `Val` into `W`  -  a constant via `MOVLW`
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
                    // stay loud panics (their address is a link-time
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
                let (a, f) = self.operand(addr);
                let bank = if a == 0 { "A" } else { "B" };
                self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    // A function's address is a link-time label literal:
                    // byte 0 = LOW(g), byte 1 = HIGH(g) (epic-cc#73).
                    let lit = if offset == 0 { "LOW" } else { "HIGH" };
                    self.emit(format!("    MOVLW {lit}({g})"));
                } else {
                    let addr = self.val_addr(v).direct() + u16::from(offset);
                    let (a, f) = self.operand(addr);
                    let bank = if a == 0 { "A" } else { "B" };
                    self.emit(format!("    MOVF 0x{f:03X},W,{bank}"));
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
    /// the P6 headline. The result is the low `bytes` bytes of the product,
    /// written to the retval region:
    /// - 1 byte: one MULWF, result = PRODL.
    /// - 2 bytes: P00 (shift 0) and P01 + P10 (shift 8) contribute to the
    ///   low 16 bits; P11 (shift 16) is dropped.
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

    /// The restoring-division loop body: shifts `num` left one bit per
    /// iteration (the quotient builds in its vacated bits), accumulates the
    /// partial remainder in `rem_base..rem_base+rem_bytes`, subtracts `den`
    /// (`SUBFWB` for the borrow chain, the exact semantics PIC14
    /// synthesized with its BTFSS/INCFSZ dance), and restores by adding
    /// back when the subtraction borrowed. `cnt` counts `8*den_bytes`
    /// iterations. `BNC`/`BRA` real branches mean the frame needs no
    /// single-GPR-bank constraint (P6 ruling). Shared by the unsigned
    /// (`emit_divmod`) and signed (`emit_sdivmod`) wrappers.
    ///
    /// `rem_bytes` is the remainder width, `den_bytes` the divisor width:
    /// the u8 case uses a 2-byte remainder ("the 8-bit rem shift can
    /// carry", the PIC14 layout contract) with a 1-byte divisor: the
    /// divisor's implicit high byte is 0, folded with `MOVLW 0` +
    /// `SUBFWB`/`ADDWFC`. u16 and u32 use equal widths.
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
        self.emit(format!("    MOVF 0x{y:03X},W,A"));
        self.emit(format!("    XORWF 0x{x:03X},F,A"));
        self.emit(format!("    MOVF 0x{x:03X},W,A"));
        self.emit(format!("    XORWF 0x{y:03X},F,A"));
        self.emit(format!("    MOVF 0x{y:03X},W,A"));
        self.emit(format!("    XORWF 0x{x:03X},F,A"));
    }

    fn emit_f32_extract(&mut self, slot: u16, sign: u16, exp: u16, mant: u16, flip: bool) {
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 3));
        self.emit("    ANDLW 0x80".to_string());
        if flip {
            self.emit("    XORLW 0x80".to_string());
        }
        self.emit(format!("    MOVWF 0x{sign:03X},A"));
        // exp = (b3 & 0x7F) << 1 | (b2 >> 7)
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{exp:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{exp:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", slot + 2));
        self.emit(format!("    BSF 0x{exp:03X}, 0,A"));
        // mant = b0, b1, (b2 & 0x7F) | 0x80 (the implicit bit, except for
        // a denormal, exp 0, which has no implicit bit).
        self.emit(format!("    MOVF 0x{:03X},W,A", slot));
        self.emit(format!("    MOVWF 0x{:03X},A", mant));
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 1));
        self.emit(format!("    MOVWF 0x{:03X},A", mant + 1));
        self.emit_f32_mant_hi(slot);
        self.emit(format!("    MOVWF 0x{:03X},A", mant + 2));
        // A denormal (exp 0, fraction nonzero) aligns at the exp-1 scale:
        // its value is frac x 2^-149 = frac x 2^(1-127-23), so the
        // alignment treats it as exp 1 with the raw fraction (no implicit
        // bit). ±0 (exp 0, fraction 0) stays exp 0.
        let _l_den = self.fresh_label();
        let l_den_done = self.fresh_label();
        self.emit(format!("    MOVF 0x{exp:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", mant));
        self.emit(format!("    IORWF 0x{:03X},W,A", mant + 1));
        self.emit(format!("    IORWF 0x{:03X},W,A", mant + 2));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{exp:03X},A"));
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
        self.emit(format!("    INCF 0x{m0:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{m1:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{m2:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_renorm}:"));
        self.emit(format!("    MOVLW 0x80"));
        self.emit(format!("    MOVWF 0x{m2:03X},A"));
        self.emit(format!("    CLRF 0x{m1:03X},A"));
        self.emit(format!("    CLRF 0x{m0:03X},A"));
        self.emit(format!("    MOVLW 0x01"));
        self.emit(format!("    ADDWF 0x{e:03X},F,A"));
        self.emit(format!("{l_done}:"));
    }

    /// Assemble the result into the fixed retval region (0x71-0x74): b0 =
    /// m0, b1 = m1, b2 = (m2 & 0x7F) | (e & 1) << 7, b3 = (e >> 1) | sign.
    fn emit_f32_assemble(&mut self, sign: u16, e: u16, m0: u16, m1: u16, m2: u16) {
        let r = self.retval_lo;
        self.emit(format!("    MOVF 0x{m0:03X},W,A"));
        self.emit(format!("    MOVWF 0x{:03X},A", r));
        self.emit(format!("    MOVF 0x{m1:03X},W,A"));
        self.emit(format!("    MOVWF 0x{:03X},A", r + 1));
        self.emit(format!("    MOVLW 0x7F"));
        self.emit(format!("    ANDWF 0x{m2:03X},W,A"));
        self.emit(format!("    MOVWF 0x{:03X},A", r + 2));
        self.emit(format!("    BTFSC 0x{e:03X}, 0,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", r + 2));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit(format!("    MOVWF 0x{:03X},A", r + 3));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", r + 3));
        self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// Load `slot+2`'s fraction into W and OR the implicit bit unless the
    /// operand is a denormal (full 8-bit exponent 0, no implicit bit,
    /// issue #11). The caller stores W into the mantissa's high byte.
    fn emit_f32_mant_hi(&mut self, slot: u16) {
        let l_imp = self.fresh_label();
        let l_done = self.fresh_label();
        // denormal check: exp 0 = (b3 & 0x7F) == 0 && !(b2 bit 7)
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_imp}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", slot + 2));
        self.emit(format!("    GOTO {l_imp}"));
        // exp 0 (denormal): fraction only, no implicit bit.
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_imp}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", slot + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("{l_done}:"));
    }

    /// Emit the fixed quiet-NaN result (0x7FC00000 | sign) and RETURN.
    /// The sign is the caller's computed result sign (IEEE leaves the NaN
    /// sign unspecified; the class is what matters).
    fn emit_f32_nan(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit(format!("    CLRF 0x{:03X},A", r));
        self.emit(format!("    CLRF 0x{:03X},A", r + 1));
        self.emit("    MOVLW 0xC0".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", r + 2));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", r + 3));
        self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// Emit the fixed infinity result (0x7F800000 | sign) and RETURN.
    fn emit_f32_inf(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit(format!("    CLRF 0x{:03X},A", r));
        self.emit(format!("    CLRF 0x{:03X},A", r + 1));
        self.emit("    MOVLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", r + 2));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", r + 3));
        self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// The __add_f32 / __sub_f32 body (both operands already extracted at the
    /// contract offsets: sa@0, ea@1, ma@2-4, sb@5, eb@6, mb@7-9, stick@10
    /// (bit 0 = round, bit 1 = the OR of the bits below the 24-bit fraction
    /// window), cnt@11, ta1@12, ta2@13; `ta0` reuses the dead `eb` slot).
    /// Extract, align the smaller exponent's mantissa right by the
    /// difference (clamped to 31), then add or subtract at the larger
    /// exponent's scale (the result exponent register becomes eb).
    ///
    /// The alignment builds the lost fraction EXACTLY: each shifted-out bit
    /// is inserted at the top of a 24-bit window `ta` (an RRCF chain, the
    /// last bit out, the round bit, lands at ta2 bit 7), and the bits that
    /// overflow the window's bottom are OR'd into `stick` bit 1. The value
    /// is the 6-byte integer `ma:ta` at the result scale.
    ///
    /// The ADD path is exact: the sum never normalizes (a carry shifts
    /// right once, promoting the old round bit into the sticky), so RNE
    /// reads round = ta2 bit 7 and sticky = OR(ta below the top bit) |
    /// stick bit 1.
    ///
    /// The SUBTRACT path is exact in the same 6-byte value: the result is
    /// |a| - |b| = (ma - mb) - frac, computed as a fractional borrow
    /// (ma -= 1 and ta = 2^24 - ta, the deep OR folded into ta's LSB)
    /// followed by the plain 3-byte subtract. The 6-byte value then
    /// normalizes with a 6-byte left shift: the fraction's bits move into
    /// the mantissa one at a time, and RNE reads the guard from ta2 bit 7,
    /// the sticky from ta's low bits | stick bit 1. A single wrong RNE bit
    /// is impossible. (The earlier two-bit round/sticky model was inexact
    /// once the fraction's tail drained to a power of two: the M15 float
    /// differential found 6/2000 SIM sub mismatches, e.g. 1.0 - 0x3EFFFFFF
    /// over-rounded to 0x3F000001 instead of the RNE 0x3F000000.)
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
        // ---- IEEE specials (issue #11): NaN and infinity operands ----
        // NaN a: exp 0xFF (ea == 0xFF) && FRACTION nonzero (the extracted
        // mantissa carries the implicit bit, so inf's 0x800000 must not
        // read as a NaN, test ma2 & 0x7F | ma1 | ma0). `cnt` is dead at
        // this point (the alignment sets it later).
        self.emit(format!("    MOVF 0x{ea:03X},W,A"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nan_done}"));
        self.emit(format!("    MOVF 0x{ma0:03X},W,A"));
        self.emit(format!("    IORWF 0x{ma1:03X},W,A"));
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("    MOVF 0x{ma2:03X},W,A"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nan_done}:"));
        // NaN b
        self.emit(format!("    MOVF 0x{eb:03X},W,A"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nan_done}"));
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    IORWF 0x{mb1:03X},W,A"));
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("    MOVF 0x{mb2:03X},W,A"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_nan_done}:"));
        // inf a? (exp 0xFF, mantissa 0, the NaN checks above already
        // routed mantissa-nonzero exp-0xFF operands to l_nan).
        self.emit(format!("    MOVF 0x{ea:03X},W,A"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_inf}"));
        // a is inf: b inf? both inf -> same sign inf, opposite NaN.
        self.emit(format!("    MOVF 0x{eb:03X},W,A"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_inf}"));
        self.emit(format!("    MOVF 0x{sa:03X},W,A"));
        self.emit(format!("    XORWF 0x{sb:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_not_inf}:"));
        // a inf, b finite: result inf (a's sign).
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_not_inf}:"));
        // a finite: b inf? result inf (b's sign).
        self.emit(format!("    MOVF 0x{eb:03X},W,A"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf_done}"));
        self.emit(format!("    MOVF 0x{sb:03X},W,A"));
        self.emit(format!("    MOVWF 0x{sa:03X},A"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sa);
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sa);
        self.emit(format!("{l_inf_done}:"));
        // ---- zero operand handling ----
        self.emit(format!("    MOVF 0x{ma0:03X},W,A"));
        self.emit(format!("    IORWF 0x{ma1:03X},W,A"));
        self.emit(format!("    IORWF 0x{ma2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ma_nz}"));
        // ma == 0: mb == 0 -> +/-0, else the result is b exactly.
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    IORWF 0x{mb1:03X},W,A"));
        self.emit(format!("    IORWF 0x{mb2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_copy_b}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_copy_b}:"));
        for (dst, src) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit(format!("    MOVF 0x{src:03X},W,A"));
            self.emit(format!("    MOVWF 0x{dst:03X},A"));
        }
        self.emit(format!("    CLRF 0x{stick:03X},A"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz}:"));
        // mb == 0 (ma != 0): the result is a exactly.
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    IORWF 0x{mb1:03X},W,A"));
        self.emit(format!("    IORWF 0x{mb2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ma_nz2}"));
        self.emit(format!("    CLRF 0x{stick:03X},A"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz2}:"));
        self.emit(format!("    CLRF 0x{stick:03X},A"));
        self.emit(format!("    CLRF 0x{ta1:03X},A"));
        self.emit(format!("    CLRF 0x{ta2:03X},A"));
        // ---- swap so that a is the smaller-exponent operand ----
        self.emit(format!("    MOVF 0x{eb:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ea:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string()); // C=1 (ea >= eb) -> swap
        self.emit(format!("    GOTO {l_no_swap}"));
        for (x, y) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit(format!("{l_no_swap}:"));
        // ---- alignment: diff = eb - ea (a is the smaller exponent),
        //      clamped to 31, shift ma right. The result exponent is the
        //      LARGER one (eb): the sum/difference is at its scale, so the
        //      result-exp register becomes eb. ----
        self.emit(format!("    MOVF 0x{ea:03X},W,A"));
        self.emit(format!("    SUBWF 0x{eb:03X},W,A"));
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("    MOVF 0x{eb:03X},W,A"));
        self.emit(format!("    MOVWF 0x{ea:03X},A"));
        self.emit(format!("    CLRF 0x{ta0:03X},A")); // eb is dead; ta0 = 0
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    SUBWF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_no_clamp}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("{l_no_clamp}:"));
        self.emit(format!("    MOVF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("    GOTO {l_align_done}"));
        self.emit(format!("{l_align_loop}:"));
        // ma >>= 1; the shifted-out bit enters the TOP of the 24-bit
        // fraction window ta (the last bit out = the round, at ta2 bit 7);
        // bits pushed out the window's bottom accumulate in stick bit 1.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{ma2:03X},F,A"));
        self.emit(format!("    RRCF 0x{ma1:03X},F,A"));
        self.emit(format!("    RRCF 0x{ma0:03X},F,A"));
        self.emit(format!("    RRCF 0x{ta2:03X},F,A"));
        self.emit(format!("    RRCF 0x{ta1:03X},F,A"));
        self.emit(format!("    RRCF 0x{ta0:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{stick:03X}, 1,A"));
        self.emit(format!("    BTFSC 0x{ta2:03X}, 7,A"));
        self.emit(format!("    BSF 0x{stick:03X}, 0,A"));
        self.emit(format!("    BTFSS 0x{ta2:03X}, 7,A"));
        self.emit(format!("    BCF 0x{stick:03X}, 0,A"));
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("{l_align_done}:"));
        // ---- signs equal? add : subtract ----
        self.emit(format!("    MOVF 0x{sa:03X},W,A"));
        self.emit(format!("    XORWF 0x{sb:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub}"));
        // add: ma += mb (3-byte carry chain); a carry renormalizes
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    ADDWF 0x{ma0:03X},F,A"));
        self.emit(format!("    MOVF 0x{mb1:03X},W,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{mb1:03X},W,A"));
        self.emit(format!("    ADDWF 0x{ma1:03X},F,A"));
        self.emit(format!("    MOVF 0x{mb2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{mb2:03X},W,A"));
        self.emit(format!("    ADDWF 0x{ma2:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_add_carry}"));
        self.emit(format!("    GOTO {l_round_step}"));
        self.emit(format!("{l_add_carry}:"));
        self.emit(format!("    BTFSC 0x{stick:03X}, 0,A"));
        self.emit(format!("    BSF 0x{stick:03X}, 1,A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{ma2:03X},F,A"));
        self.emit(format!("    RRCF 0x{ma1:03X},F,A"));
        self.emit(format!("    RRCF 0x{ma0:03X},F,A"));
        self.emit(format!("    BCF 0x{stick:03X}, 0,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{stick:03X}, 0,A"));
        self.emit(format!("    BSF 0x{ma2:03X}, 7,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{ea:03X},F,A"));
        self.emit(format!("    GOTO {l_round_step}"));
        // subtract: compare ma vs mb (the sign follows the larger)
        self.emit(format!("{l_sub}:"));
        self.emit(format!("    MOVF 0x{mb2:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_b1}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b1}:"));
        self.emit(format!("    MOVF 0x{mb1:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma1:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_b0}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b0}:"));
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma0:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_cmp_frac}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        // ma == mb: |a| == |b| iff the fraction is 0, else a is larger by
        // exactly the fraction (the value is frac, sign = sa).
        self.emit(format!("{l_cmp_frac}:"));
        self.emit(format!("    MOVF 0x{ta0:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta1:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_equal_frac}"));
        self.emit(format!("    BTFSS 0x{stick:03X}, 1,A"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_sub_equal_frac}:"));
        self.emit(format!("    BTFSC 0x{stick:03X}, 1,A"));
        self.emit(format!("    BSF 0x{ta0:03X}, 0,A"));
        self.emit(format!("    CLRF 0x{ma0:03X},A"));
        self.emit(format!("    CLRF 0x{ma1:03X},A"));
        self.emit(format!("    CLRF 0x{ma2:03X},A"));
        self.emit(format!("    GOTO {l_normalize}"));
        self.emit(format!("{l_sub_swap}:"));
        for (x, y) in [(ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit(format!("    MOVF 0x{sb:03X},W,A"));
        self.emit(format!("    MOVWF 0x{sa:03X},A"));
        self.emit(format!("{l_sub_done}:"));
        // ---- fractional borrow: the exact result is (ma - mb) - frac, so
        //      for frac != 0 the integer part borrows (ma -= 1) and the
        //      fraction becomes 2^24 - ta (the deep OR folded into ta's
        //      LSB first, it is below the 24-bit window, sticky-typed).
        //      frac == 0 skips straight to the plain 3-byte subtract. ----
        self.emit(format!("    BTFSC 0x{stick:03X}, 1,A"));
        self.emit(format!("    BSF 0x{ta0:03X}, 0,A"));
        self.emit(format!("    MOVF 0x{ta0:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta1:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_no_frac}"));
        self.emit(format!("    COMF 0x{ta0:03X},F,A"));
        self.emit(format!("    COMF 0x{ta1:03X},F,A"));
        self.emit(format!("    COMF 0x{ta2:03X},F,A"));
        self.emit(format!("    INCF 0x{ta0:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{ta1:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{ta2:03X},F,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma0:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma1:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma2:03X},F,A"));
        self.emit(format!("{l_sub_borrow_done}:"));
        self.emit(format!("{l_sub_no_frac}:"));
        // ma -= mb (3-byte borrow chain)
        self.emit(format!("    MOVF 0x{mb0:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma0:03X},F,A"));
        self.emit(format!("    MOVF 0x{mb1:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{mb1:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma1:03X},F,A"));
        self.emit(format!("    MOVF 0x{mb2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{mb2:03X},W,A"));
        self.emit(format!("    SUBWF 0x{ma2:03X},F,A"));
        self.emit(format!("    GOTO {l_normalize}"));
        // ---- normalize the 6-byte value: while !(ma2 bit 7) && ea > 1:
        //      (ta:ma) <<= 1 (the fraction's bits move into the mantissa),
        //      ea--. Only the subtract path reaches this (a sum never
        //      normalizes). The loop stops at ea == 1, NOT 0: a denormal
        //      result is the raw fraction at the exp-1 scale (value =
        //      frac x 2^-149), and the denormal conversion below drops it
        //      to exp 0. Stopping at 0 would leave ma = 2 x frac, doubling
        //      the stored value. ----
        self.emit(format!("{l_normalize}:"));
        self.emit(format!("    MOVF 0x{ma2:03X},W,A"));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit(format!("    MOVF 0x{ea:03X},W,A"));
        self.emit("    SUBLW 0x01".to_string()); // ea == 1 -> stop
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{ta0:03X},F,A"));
        self.emit(format!("    RLCF 0x{ta1:03X},F,A"));
        self.emit(format!("    RLCF 0x{ta2:03X},F,A"));
        self.emit(format!("    RLCF 0x{ma0:03X},F,A"));
        self.emit(format!("    RLCF 0x{ma1:03X},F,A"));
        self.emit(format!("    RLCF 0x{ma2:03X},F,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ea:03X},F,A"));
        self.emit(format!("    GOTO {l_normalize}"));
        // ---- subtract-path guard: the top fraction bit (ta2 bit 7) ----
        self.emit(format!("{l_sub_guard}:"));
        self.emit(format!("    BTFSC 0x{ta2:03X}, 7,A"));
        self.emit(format!("    BSF 0x{stick:03X}, 0,A"));
        self.emit(format!("    BTFSS 0x{ta2:03X}, 7,A"));
        self.emit(format!("    BCF 0x{stick:03X}, 0,A"));
        // ---- RNE: round up iff round && (sticky || mantissa LSB), with
        //      sticky = OR(ta below the top bit) | stick bit 1 (the deep
        //      OR): the add path's round/sticky are equivalent (round =
        //      ta2 bit 7 = the last shifted-out bit; a sum carry promotes
        //      the old round into stick bit 1). ----
        // ---- denormal result: exp 1 with the top mantissa bit clear (the
        //      sum/difference of denormals, or a normal that underflowed)
        //      converts to exp 0: the mantissa is already the raw
        //      fraction (no implicit bit), and the assemble emits it as-is
        //      for e == 0. A rounded-up 0x800000 (the smallest normal)
        //      keeps exp 1. Both paths reach here: the add path directly
        //      (a denormal sum never normalizes), the sub path after its
        //      normalize loop stops at ea == 1. ----
        let l_den_conv = self.fresh_label();
        let l_den_done = self.fresh_label();
        self.emit(format!("{l_round_step}:"));
        self.emit(format!("    BTFSS 0x{stick:03X}, 0,A"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("    MOVF 0x{ta0:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta1:03X},W,A"));
        self.emit(format!("    IORWF 0x{ta2:03X},W,A"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{stick:03X}, 1,A"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{ma0:03X}, 0,A"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(ma0, ma1, ma2, ea);
        self.emit(format!("{l_den_conv}:"));
        self.emit(format!("    MOVF 0x{ma2:03X},W,A"));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    MOVF 0x{ea:03X},W,A"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    CLRF 0x{ea:03X},A"));
        self.emit(format!("{l_den_done}:"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sa, ea, ma0, ma1, ma2);
        // ---- zero result: sign = sa & sb, exp 0, mantissa 0 ----
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    BTFSS 0x{sa:03X}, 7,A"));
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit(format!("    BTFSS 0x{sb:03X}, 7,A"));
        self.emit(format!("    GOTO {l_zs_clear}"));
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit(format!("{l_zs_clear}:"));
        self.emit(format!("    BCF 0x{sa:03X}, 7,A"));
        self.emit(format!("{l_zs_done}:"));
        self.emit(format!("    CLRF 0x{ea:03X},A"));
        self.emit(format!("    CLRF 0x{ma0:03X},A"));
        self.emit(format!("    CLRF 0x{ma1:03X},A"));
        self.emit(format!("    CLRF 0x{ma2:03X},A"));
        self.emit(format!("    GOTO {l_assemble}"));
    }

    /// The __mul_f32 body (scratch: sign@0, e@1-2 = the 16-bit result exp,
    /// bk@3-5 = the multiplier backup, cnt@6 = 24, m@7-10 = the product's
    /// top 25 bits (P >> 23), low@11-13 = the exact mod-2^23 product low
    /// bits; the a/b param slots hold the shifted multiplicand addend and
    /// the low-part register). The 24x24 shift-add runs 24 iterations (the
    /// AN526 pattern): per set multiplier bit, low += (ma << i) mod 2^23
    /// (exact, its carry feeds m) and m += (ma >> (23-i)); then normalize
    /// (bit 47 of the product -> exp+1), round RNE (guard = P bit 22 /
    /// shifted-out bit, sticky = the low bits), assemble.
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
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit(format!("    XORWF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{sign:03X},A"));
        // e = ea + eb - 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8 + eb8 (9 bits: S_lo +
        // C0); e_lo = S_lo + 0x81 (C1); e_hi = C0 - borrow (borrow = !C1).
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{low0:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{low0:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    BSF 0x{low0:03X}, 0,A")); // ea8
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{low1:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{low1:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    BSF 0x{low1:03X}, 0,A")); // eb8
                                                         // A nonzero exp-zero operand aligns at exp 1 with its raw fraction.
        self.emit(format!("    MOVF 0x{low0:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{low0:03X},A"));
        self.emit(format!("{l_a_exp_done}:"));
        self.emit(format!("    MOVF 0x{low1:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{low1:03X},A"));
        self.emit(format!("{l_b_exp_done}:"));
        self.emit(format!("    MOVF 0x{low1:03X},W,A"));
        self.emit(format!("    ADDWF 0x{low0:03X},W,A"));
        self.emit(format!("    MOVWF 0x{low0:03X},A"));
        self.emit(format!("    CLRF 0x{m3:03X},A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{m3:03X}, 0,A")); // m3 bit 0 = C0
        self.emit("    MOVLW 0x81".to_string());
        self.emit(format!("    ADDWF 0x{low0:03X},W,A"));
        self.emit(format!("    MOVWF 0x{e:03X},A"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_ehi_c1clear}"));
        self.emit(format!("    BTFSC 0x{m3:03X}, 0,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_c1clear}:"));
        self.emit(format!("    BTFSC 0x{m3:03X}, 0,A"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit(format!("    MOVWF 0x{:03X},A", e + 1));
        // NaN and infinity classification uses the raw exponent/fraction.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_not_ff}:"));
        // Finite zero operands produce signed zero; check the complete raw
        // fraction so denormals are not mistaken for zero.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:03X},W,A"));
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
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", pa + 2));
        self.emit(format!("    GOTO {l_a_mant_done}"));
        self.emit(format!("{l_a_mant_implicit}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", pa + 2));
        self.emit(format!("{l_a_mant_done}:"));
        // bk = mb copy (the multiplier, shifted to test bits)
        // bk = mb copy (the multiplier, shifted to test bits)
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    MOVWF 0x{bk0:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{bk1:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{bk2:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit(format!("    GOTO {l_b_mant_done}"));
        self.emit(format!("{l_b_mant_implicit}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{bk2:03X},A"));
        self.emit(format!("{l_b_mant_done}:"));
        // la = the low-part addend, maintained as la_{i+1} = (la_i >> 1) |
        // (ma bit i << 22): the correct low contribution (ma mod 2^i) <<
        // (23-i) at iteration i (testing mb bit 23-i). Starts at 0 (i=0:
        // (ma mod 1) << 23 = 0). (The M15 float probe: an earlier attempt
        // copied ma into the slot: (ma mod 2^23) << i, which is a
        // different, wrong addend that broke every inexact product.)
        self.emit(format!("    CLRF 0x{:03X},A", pb));
        self.emit(format!("    CLRF 0x{:03X},A", pb + 1));
        self.emit(format!("    CLRF 0x{:03X},A", pb + 2));
        for addr in [m0, m1, m2, m3, low0, low1, low2] {
            self.emit(format!("    CLRF 0x{addr:03X},A"));
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("{l_loop}:"));
        // test the multiplier bit (bk <<= 1, C = the bit)
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{bk0:03X},F,A"));
        self.emit(format!("    RLCF 0x{bk1:03X},F,A"));
        self.emit(format!("    RLCF 0x{bk2:03X},F,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_skip}"));
        // low += la (3-byte): the FIRST byte adds WITHOUT a carry-in: the
        // C at this point is the tested multiplier bit (set by the RLCF bk
        // chain), not a carry, so the BTFSC/INCFSZ carry-in would add a
        // spurious +1 per set-bit iteration (the M15 float probe found the
        // low sum came out one per set bit too high). Bytes 1-2 take the
        // carry from the previous byte's add.
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    ADDWF 0x{low0:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 1));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", pb + 1));
        self.emit(format!("    ADDWF 0x{low1:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", pb + 2));
        self.emit(format!("    ADDWF 0x{low2:03X},F,A"));
        // m += addend (4-byte) + the low's carry-out: the carry into m is
        // BIT 23 of the 24-bit low sum (the top byte's bit 7), NOT the
        // byte carry-out (bit 24): the M15 float probe found the original
        // tested STATUS C, so a sum with bit 23 set but no byte overflow
        // (e.g. 0x700003 + 0x160000 = 0x860003) lost its carry into m and
        // every inexact product came out one 2^23 short. The carry path
        // also masks bit 23 out of low (low is mod 2^23).
        self.emit(format!("    BTFSC 0x{low2:03X}, 7,A"));
        self.emit(format!("    GOTO {l_carry_in}"));
        self.emit(format!("    GOTO {l_no_carry}"));
        self.emit(format!("{l_carry_in}:"));
        self.emit(format!("    BCF 0x{low2:03X}, 7,A"));
        self.emit(format!("    INCF 0x{m0:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{m1:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{m2:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{m3:03X},F,A"));
        self.emit(format!("{l_no_carry}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    ADDWF 0x{m0:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 1));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", pa + 1));
        self.emit(format!("    ADDWF 0x{m1:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", pa + 2));
        self.emit(format!("    ADDWF 0x{m2:03X},F,A"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{m3:03X},F,A"));
        self.emit(format!("{l_skip}:"));
        // la = (la >> 1) | (ma bit i << 22): pa bit 0 is ma bit i (pa has
        // been shifted right i times), so the new bit enters at la bit 22.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pb + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pb + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pb));
        self.emit(format!("    BTFSC 0x{:03X}, 0,A", pa));
        self.emit(format!("    BSF 0x{:03X}, 6,A", pb + 2));
        // addend >>= 1 (pa = ma >> (i+1))
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa));
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_loop}"));
        // Convert the product into a unified 47-bit register. A renormalized
        // product already has the correct scale after m >>= 1; otherwise the
        // leading zero m3 is dropped by shifting P left once.
        self.emit(format!("    BTFSC 0x{m3:03X}, 0,A"));
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        // The m bytes already occupy P bits 46..23; shift only the low
        // 23-bit portion so P bit 22 becomes the unified guard bit.
        self.emit(format!("    RLCF 0x{low0:03X},F,A"));
        self.emit(format!("    RLCF 0x{low1:03X},F,A"));
        self.emit(format!("    RLCF 0x{low2:03X},F,A"));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_renorm}:"));
        // m >>= 1; the old m bit 0 is the unified register's guard bit.
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    BTFSC 0x{m3:03X}, 0,A"));
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{m2:03X},F,A"));
        self.emit(format!("    RRCF 0x{m1:03X},F,A"));
        self.emit(format!("    RRCF 0x{m0:03X},F,A"));
        self.emit(format!("    BCF 0x{low2:03X}, 7,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{low2:03X}, 7,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{e:03X},F,A"));
        self.emit(format!("{l_norm_check}:"));
        // First handle e < 1 (including the negative 16-bit exponents of
        // tiny products), then left-normalize while e > 1.
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", e + 1));
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", e + 1));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit(format!("{l_norm_left}:"));
        self.emit(format!("    BTFSC 0x{m2:03X}, 7,A"));
        self.emit(format!("    GOTO {l_extract}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", e + 1));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_norm_left_shift}"));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_extract}"));
        self.emit(format!("{l_norm_left_shift}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{low0:03X},F,A"));
        self.emit(format!("    RLCF 0x{low1:03X},F,A"));
        self.emit(format!("    RLCF 0x{low2:03X},F,A"));
        self.emit(format!("    RLCF 0x{m0:03X},F,A"));
        self.emit(format!("    RLCF 0x{m1:03X},F,A"));
        self.emit(format!("    RLCF 0x{m2:03X},F,A"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:03X},F,A", e + 1));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_norm_right}:"));
        self.emit(format!("    BTFSC 0x{low0:03X}, 0,A"));
        self.emit(format!("    BSF 0x{m3:03X}, 1,A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{m2:03X},F,A"));
        self.emit(format!("    RRCF 0x{m1:03X},F,A"));
        self.emit(format!("    RRCF 0x{m0:03X},F,A"));
        self.emit(format!("    RRCF 0x{low2:03X},F,A"));
        self.emit(format!("    RRCF 0x{low1:03X},F,A"));
        self.emit(format!("    RRCF 0x{low0:03X},F,A"));
        self.emit(format!("    INCF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{:03X},F,A", e + 1));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_extract}:"));
        // guard = unified bit 23; sticky = unified bits 0..22 plus any bits
        // shifted out while producing a denormal.
        self.emit(format!("    BTFSS 0x{low2:03X}, 7,A"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    ANDWF 0x{low2:03X},W,A"));
        self.emit(format!("    IORWF 0x{low1:03X},W,A"));
        self.emit(format!("    IORWF 0x{low0:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{m3:03X}, 1,A"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{m0:03X}, 0,A"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(m0, m1, m2, e);
        self.emit(format!("{l_den_conv}:"));
        // exp 1 with a clear mantissa top is the denormal encoding.
        self.emit(format!("    BTFSC 0x{m2:03X}, 7,A"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", e + 1));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    CLRF 0x{e:03X},A"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, m0, m1, m2);
        // the +/-0 result (zero operand): sign | 0
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    CLRF 0x{:03X},A", self.retval_lo));
        self.emit(format!("    CLRF 0x{:03X},A", self.retval_lo + 1));
        self.emit(format!("    CLRF 0x{:03X},A", self.retval_lo + 2));
        self.emit(format!("    CLRF 0x{:03X},A", self.retval_lo + 3));
        self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", self.retval_lo + 3));
        self.emit("    RETURN".to_string());
    }

    /// One restoring-division compare/subtract/restore step: rem (4 bytes)
    /// -= den (3 bytes, the top byte is implicitly 0) with the borrow
    /// folds; on underflow (rem < den) add den back. The final C is the
    /// quotient bit: the caller's branch lands at `l_restore` when clear and
    /// sets the bit at `qbit` bit 0 otherwise; `l_next` resumes after.
    fn emit_f32_div_step(&mut self, rem: u16, den: u16, qbit: u16, l_restore: &str, l_next: &str) {
        self.emit(format!("    MOVF 0x{den:03X},W,A"));
        self.emit(format!("    SUBWF 0x{rem:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", den + 1));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", den + 1));
        self.emit(format!("    SUBWF 0x{:03X},F,A", rem + 1));
        self.emit(format!("    MOVF 0x{:03X},W,A", den + 2));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", den + 2));
        self.emit(format!("    SUBWF 0x{:03X},F,A", rem + 2));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:03X},F,A", rem + 3));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_restore}"));
        self.emit(format!("    BSF 0x{qbit:03X}, 0,A"));
        self.emit(format!("    GOTO {l_next}"));
        self.emit(format!("{l_restore}:"));
        self.emit(format!("    MOVF 0x{den:03X},W,A"));
        self.emit(format!("    ADDWF 0x{rem:03X},F,A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", den + 1));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", den + 1));
        self.emit(format!("    ADDWF 0x{:03X},F,A", rem + 1));
        self.emit(format!("    MOVF 0x{:03X},W,A", den + 2));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCFSZ 0x{:03X},W,A", den + 2));
        self.emit(format!("    ADDWF 0x{:03X},F,A", rem + 2));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{:03X},F,A", rem + 3));
        self.emit(format!("{l_next}:"));
    }

    /// The __div_f32 body (scratch: sign@0, e@1-2 = the 16-bit result exp,
    /// rem@3-6 = the partial remainder, den@7-9 = the denominator copy, cnt@10,
    /// spare@11). The numerator lives in the a param slot. 24 restoring
    /// iterations give floor(ma/mb) (0/1, ma >= mb bumps e) with rem = ma
    /// mod mb; 25 more iterations extend the quotient to the mantissa +
    /// guard (the remainder at the end is the sticky). Div-by-zero (mb == 0)
    /// -> +/-infinity (0x7F800000, deterministic, documented); ma == 0 ->
    /// +/-0. The e is clamped by the byte arithmetic for the out-of-range
    /// cases (deterministic; the acceptance stays in the normal range).
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
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit(format!("    XORWF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{sign:03X},A"));
        // e = ea - eb + 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8 - eb8 (S_lo + borrow B);
        // e_lo = S_lo + 0x7F (C1); e_hi = C1 - B.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{spare:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    BSF 0x{spare:03X}, 0,A")); // ea8
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{e:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    BSF 0x{e:03X}, 0,A")); // eb8
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit(format!("    SUBWF 0x{spare:03X},W,A"));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    CLRF 0x{rem3:03X},A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{rem3:03X}, 0,A")); // rem3 bit 0 = borrow B
        self.emit("    ADDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:03X},A"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_ehi_b}"));
        self.emit(format!("    BTFSC 0x{rem3:03X}, 0,A"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_b}:"));
        self.emit(format!("    BTFSS 0x{rem3:03X}, 0,A"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit(format!("    MOVWF 0x{:03X},A", e + 1));
        // IEEE class dispatch, using the raw exponent and complete fraction.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_not_ff}:"));
        // finite zero checks include the complete fraction
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sign);
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sign);
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    CLRF 0x{r:03X},A"));
        self.emit(format!("    CLRF 0x{:03X},A", r + 1));
        self.emit(format!("    CLRF 0x{:03X},A", r + 2));
        self.emit(format!("    CLRF 0x{:03X},A", r + 3));
        self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", r + 3));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_b_nz}:"));
        // Denormals begin at the exp-1 alignment scale (effective e8 = 1).
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit(format!("    INCF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{:03X},F,A", e + 1));
        self.emit(format!("{l_exp_a_done}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:03X},F,A", e + 1));
        self.emit(format!("{l_exp_b_done}:"));
        // Build raw/implicit mantissas, then normalize denormals to bit 23.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", pa + 2));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit(format!("    GOTO {l_a_ready}"));
        self.emit(format!("{l_a_imp}:"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("{l_a_ready}:"));
        self.emit(format!("    CLRF 0x{rem3:03X},A"));
        self.emit(format!("{l_norm_a}:"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_norm_a_done}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{:03X},F,A", pa));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    INCF 0x{rem3:03X},F,A"));
        self.emit(format!("    GOTO {l_norm_a}"));
        self.emit(format!("{l_norm_a_done}:"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:03X},A", pb + 2));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit(format!("    GOTO {l_b_ready}"));
        self.emit(format!("{l_b_imp}:"));
        self.emit(format!("    BSF 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("{l_b_ready}:"));
        self.emit(format!("    CLRF 0x{cnt:03X},A"));
        self.emit(format!("{l_norm_b}:"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_norm_b_done}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{:03X},F,A", pb));
        self.emit(format!("    RLCF 0x{:03X},F,A", pb + 1));
        self.emit(format!("    RLCF 0x{:03X},F,A", pb + 2));
        self.emit(format!("    INCF 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_norm_b}"));
        self.emit(format!("{l_norm_b_done}:"));
        self.emit(format!("    MOVF 0x{rem3:03X},W,A"));
        self.emit(format!("    SUBWF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_e_sub_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:03X},F,A", e + 1));
        self.emit(format!("{l_e_sub_done}:"));
        self.emit(format!("    MOVF 0x{cnt:03X},W,A"));
        self.emit(format!("    ADDWF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    INCF 0x{:03X},F,A", e + 1));
        // denominator copy, now normalized to [2^23, 2^24).
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    MOVWF 0x{den0:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    MOVWF 0x{den1:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit(format!("    MOVWF 0x{den2:03X},A"));
        // ---- 24 restoring iterations: num <<= 1; rem = rem << 1 | C;
        //      if rem >= den set the quotient bit else restore ----
        for addr in [rem0, rem1, rem2, rem3] {
            self.emit(format!("    CLRF 0x{addr:03X},A"));
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("{l_loop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{:03X},F,A", pa));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RLCF 0x{rem0:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem1:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem2:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem3:03X},F,A"));
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_restore, &l_next);
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_loop}"));
        // Save floor(ma/mb) (0/1, ma >= mb) before the mantissa
        // accumulator clears pa.
        self.emit(format!("    CLRF 0x{spare:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit("    ANDLW 0x01".to_string());
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_qsave}"));
        self.emit(format!("    BSF 0x{spare:03X}, 0,A"));
        self.emit(format!("{l_qsave}:"));
        // ---- 25 more iterations: the mantissa + guard, with the sticky in
        //      the remainder ----
        for addr in [pa, pa + 1, pa + 2, pa + 3] {
            self.emit(format!("    CLRF 0x{addr:03X},A"));
        }
        self.emit("    MOVLW 0x19".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("{l_floop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{:03X},F,A", pa));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RLCF 0x{:03X},F,A", pa + 3));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{rem0:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem1:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem2:03X},F,A"));
        self.emit(format!("    RLCF 0x{rem3:03X},F,A"));
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_frestore, &l_fnext);
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_floop}"));
        // The mantissa: q = ma/mb in [0.5, 2). The fraction loop's 25 bits
        // are q's bits 2^-1..2^-25 (pa: f1 at bit 24 .. f25 at bit 0), with
        // the remainder as the sticky. For q < 1 (floor(q) == 0) the
        // mantissa = f1..f24 (f1 = 1 at bit 23) with exp-1; for q >= 1 the
        // mantissa = 1.f1..f23 = 0x800000 | (pa >> 2) with the guard f24
        // (pa bit 1) and the sticky f25 (pa bit 0) | rem. e = ea - eb + 127.
        self.emit(format!("    BTFSC 0x{spare:03X}, 0,A"));
        self.emit(format!("    GOTO {l_ge1}"));
        // q < 1: mantissa = pa >> 1; guard = old pa bit 0; sticky = rem;
        // exp -= 1.
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:03X},F,A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    BTFSC 0x{:03X}, 0,A", pa + 3));
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa));
        self.emit(format!("    CLRF 0x{spare:03X},A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{spare:03X}, 0,A")); // guard
        self.emit(format!("    MOVF 0x{rem0:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem1:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem2:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem3:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("    BSF 0x{spare:03X}, 1,A")); // sticky
        self.emit(format!("    GOTO {l_round}"));
        // q >= 1: mantissa = 0x800000 | (pa >> 2); guard = old pa bit 1;
        // sticky = old pa bit 0 | rem.
        self.emit(format!("{l_ge1}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    BTFSC 0x{:03X}, 0,A", pa + 3));
        self.emit("    BSF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa));
        self.emit(format!("    CLRF 0x{spare:03X},A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{spare:03X}, 1,A")); // old bit 0 -> sticky
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{spare:03X}, 0,A")); // guard = old bit 1
        self.emit(format!("    BSF 0x{:03X}, 7,A", pa + 2)); // the leading 1
        self.emit(format!("    MOVF 0x{rem0:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem1:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem2:03X},W,A"));
        self.emit(format!("    IORWF 0x{rem3:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("    BSF 0x{spare:03X}, 1,A")); // sticky |= rem
                                                          // RNE: guard (spare bit 0) && (sticky (spare bit 1) || mantissa LSB)
        self.emit(format!("{l_round}:"));
        // Shift a subnormal result right while e < 1, preserving guard and
        // sticky for the final round-to-nearest-even decision.
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", e + 1));
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_test}"));
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit(format!("{l_den_shift}:"));
        self.emit(format!("    BTFSC 0x{spare:03X}, 0,A"));
        self.emit(format!("    BSF 0x{spare:03X}, 1,A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 2));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa + 1));
        self.emit(format!("    RRCF 0x{:03X},F,A", pa));
        self.emit(format!("    BCF 0x{spare:03X}, 0,A"));
        self.emit("    BTFSC 0xFD8,0,A".to_string());
        self.emit(format!("    BSF 0x{spare:03X}, 0,A"));
        self.emit(format!("    INCF 0x{e:03X},F,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    INCF 0x{:03X},F,A", e + 1));
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("{l_round_test}:"));
        self.emit(format!("    BTFSS 0x{spare:03X}, 0,A"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("    BTFSC 0x{spare:03X}, 1,A"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{:03X}, 0,A", pa));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(pa, pa + 1, pa + 2, e);
        self.emit(format!("{l_den_conv}:"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", e + 1));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    CLRF 0x{e:03X},A"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, pa, pa + 1, pa + 2);
    }

    /// The __uitofp_f32 body (scratch: cnt@0, e@1-2, guard@3, stick@4,
    /// spare@5-7; `sign_src` = the byte holding the sign for __sitofp_f32,
    /// or None for the unsigned +0). Leading-1 search: shift the value left
    /// until bit 31 set, counting; e = 127 + 31 - shifts; the mantissa is
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
        self.emit(format!("    MOVF 0x{:03X},W,A", val));
        self.emit(format!("    IORWF 0x{:03X},W,A", val + 1));
        self.emit(format!("    IORWF 0x{:03X},W,A", val + 2));
        self.emit(format!("    IORWF 0x{:03X},W,A", val + 3));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit(format!("    CLRF 0x{r:03X},A"));
        self.emit(format!("    CLRF 0x{:03X},A", r + 1));
        self.emit(format!("    CLRF 0x{:03X},A", r + 2));
        self.emit(format!("    CLRF 0x{:03X},A", r + 3));
        if sign_src.is_some() {
            self.emit(format!("    BTFSC 0x{sign:03X}, 7,A"));
            self.emit(format!("    BSF 0x{:03X}, 7,A", r + 3));
        }
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_nz}:"));
        if sign_src.is_none() {
            self.emit(format!("    CLRF 0x{sign:03X},A"));
        }
        self.emit(format!("    CLRF 0x{cnt:03X},A"));
        self.emit(format!("{l_loop}:"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", val + 3));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        for i in 0..4 {
            self.emit(format!("    RLCF 0x{:03X},F,A", val + i));
        }
        self.emit(format!("    INCF 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_zero}:"));
        // e = 158 - cnt; the mantissa is val+1..val+3 (bit 23 = val3 bit 7)
        self.emit(format!("    MOVF 0x{cnt:03X},W,A"));
        self.emit("    SUBLW 0x9E".to_string()); // 158 - cnt
        self.emit(format!("    MOVWF 0x{e:03X},A"));
        self.emit(format!("    CLRF 0x{:03X},A", e + 1));
        self.emit(format!("    MOVF 0x{:03X},W,A", val));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{guard:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", val));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{stick:03X},A"));
        // RNE: guard && (sticky || mantissa LSB)
        self.emit(format!("    BTFSS 0x{guard:03X}, 7,A"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{stick:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{:03X}, 0,A", val + 1));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(val + 1, val + 2, val + 3, e);
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, val + 1, val + 2, val + 3);
    }

    /// The __fptoui_f32 / __fptosi_f32 body (scratch: e@0, cnt@1, m@2-4,
    /// sign@5, spare@6 = the result's 4th byte). The biased exponent maps to
    /// a mantissa shift: right by 150 - e (truncating; e <= 150), left by
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
        self.emit(format!("    MOVF 0x{:03X},W,A", val + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:03X},A"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{e:03X},F,A"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", val + 2));
        self.emit(format!("    BSF 0x{e:03X}, 0,A"));
        if signed {
            self.emit(format!("    MOVF 0x{:03X},W,A", val + 3));
            self.emit("    ANDLW 0x80".to_string());
            self.emit(format!("    MOVWF 0x{sign:03X},A"));
        }
        // e == 0 -> result 0 (the sign is dropped for zero)
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit(format!("    CLRF 0x{m0:03X},A"));
        self.emit(format!("    CLRF 0x{m1:03X},A"));
        self.emit(format!("    CLRF 0x{m2:03X},A"));
        self.emit(format!("    CLRF 0x{m3:03X},A"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_nz}:"));
        // m = the 24-bit mantissa with the implicit bit
        self.emit(format!("    MOVF 0x{:03X},W,A", val));
        self.emit(format!("    MOVWF 0x{m0:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", val + 1));
        self.emit(format!("    MOVWF 0x{m1:03X},A"));
        self.emit(format!("    MOVF 0x{:03X},W,A", val + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{m2:03X},A"));
        self.emit(format!("    CLRF 0x{m3:03X},A"));
        // cnt = 150 - e
        self.emit(format!("    MOVF 0x{e:03X},W,A"));
        self.emit("    SUBLW 0x96".to_string()); // 150 - e
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_left}"));
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        // clamp the right count to 31 (the 24-bit mantissa is zero beyond)
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    SUBWF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_rdone}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        self.emit(format!("{l_rdone}:"));
        self.emit(format!("    MOVF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_rloop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RRCF 0x{m2:03X},F,A"));
        self.emit(format!("    RRCF 0x{m1:03X},F,A"));
        self.emit(format!("    RRCF 0x{m0:03X},F,A"));
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_left}:"));
        // cnt = e - 150 (W = 150 - e, negate)
        self.emit("    SUBLW 0x00".to_string());
        self.emit(format!("    MOVWF 0x{cnt:03X},A"));
        // overflow clamp: fptoui cnt > 8 (e >= 159); fptosi cnt >= 8 (e >= 158)
        if signed {
            self.emit("    MOVLW 0x08".to_string());
        } else {
            self.emit("    MOVLW 0x09".to_string());
        }
        self.emit(format!("    SUBWF 0x{cnt:03X},W,A"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_lloop}"));
        if signed {
            self.emit(format!("    BTFSS 0x{sign:03X}, 7,A"));
            self.emit(format!("    GOTO {l_posclamp}"));
            self.emit(format!("    CLRF 0x{m0:03X},A"));
            self.emit(format!("    CLRF 0x{m1:03X},A"));
            self.emit(format!("    CLRF 0x{m2:03X},A"));
            self.emit("    MOVLW 0x80".to_string());
            self.emit(format!("    MOVWF 0x{m3:03X},A"));
            self.emit(format!("    GOTO {l_store2}"));
            self.emit(format!("{l_posclamp}:"));
            self.emit("    MOVLW 0xFF".to_string());
            self.emit(format!("    MOVWF 0x{m0:03X},A"));
            self.emit(format!("    MOVWF 0x{m1:03X},A"));
            self.emit(format!("    MOVWF 0x{m2:03X},A"));
            self.emit("    MOVLW 0x7F".to_string());
            self.emit(format!("    MOVWF 0x{m3:03X},A"));
        } else {
            self.emit("    MOVLW 0xFF".to_string());
            self.emit(format!("    MOVWF 0x{m0:03X},A"));
            self.emit(format!("    MOVWF 0x{m1:03X},A"));
            self.emit(format!("    MOVWF 0x{m2:03X},A"));
            self.emit(format!("    MOVWF 0x{m3:03X},A"));
        }
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_lloop}:"));
        self.emit("    BCF 0xFD8,0,A".to_string());
        self.emit(format!("    RLCF 0x{m0:03X},F,A"));
        self.emit(format!("    RLCF 0x{m1:03X},F,A"));
        self.emit(format!("    RLCF 0x{m2:03X},F,A"));
        self.emit(format!("    RLCF 0x{m3:03X},F,A"));
        self.emit(format!("    DECFSZ 0x{cnt:03X},F,A"));
        self.emit(format!("    GOTO {l_lloop}"));
        self.emit(format!("{l_store2}:"));
        if signed {
            // negate the 4-byte result for a negative input (truncation is
            // toward zero, the negate of 0 is 0)
            self.emit(format!("    BTFSS 0x{sign:03X}, 7,A"));
            self.emit(format!("    GOTO {l_store}"));
            for addr in [m0, m1, m2, m3] {
                self.emit(format!("    COMF 0x{addr:03X},F,A"));
            }
            self.emit(format!("    INCF 0x{m0:03X},F,A"));
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit(format!("    INCF 0x{m1:03X},F,A"));
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit(format!("    INCF 0x{m2:03X},F,A"));
            self.emit("    BTFSC 0xFD8,2,A".to_string());
            self.emit(format!("    INCF 0x{m3:03X},F,A"));
            self.emit(format!("{l_store}:"));
        }
        for (i, addr) in [m0, m1, m2, m3].iter().enumerate() {
            self.emit(format!("    MOVF 0x{addr:03X},W,A"));
            self.emit(format!("    MOVWF 0x{:03X},A", r + i as u16));
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
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{:03X},W,A", pa + 1));
        self.emit(format!("    IORWF 0x{:03X},W,A", pa));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_a_done}:"));
        // NaN b
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{:03X},W,A", pb + 1));
        self.emit(format!("    IORWF 0x{:03X},W,A", pb));
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_b_done}:"));
        // both zero (full 8-bit exp == 0, any signs) -> equal. The exponent's
        // LSB lives in b2 bit 7, so the (b3 & 0x7F) test alone swallows the
        // smallest NORMALs (8-bit exp 1: 0x00800000..0x00FFFFFF): skip the
        // zero path when b2 bit 7 is set, mirroring the mul/div zero checks.
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pa + 2));
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    MOVF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    BTFSC 0x{:03X}, 7,A", pb + 2));
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit(format!("{l_az_done}:"));
        // signs differ? a negative, b positive -> a < b (1); else a > b (2).
        // Mask the XOR to bit 7 (the exponent bits must not pollute it).
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit(format!("    XORWF 0x{:03X},W,A", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_sign_diff}"));
        // same sign: save a's sign, clear the sign bits, compare magnitudes
        self.emit(format!("    MOVF 0x{:03X},W,A", pa + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{tmp1:03X},A"));
        self.emit(format!("    BCF 0x{:03X}, 7,A", pa + 3));
        self.emit(format!("    BCF 0x{:03X}, 7,A", pb + 3));
        // equality: OR-accumulate the byte XORs into tmp0
        self.emit(format!("    MOVF 0x{:03X},W,A", pa));
        self.emit(format!("    XORWF 0x{:03X},W,A", pb));
        self.emit(format!("    MOVWF 0x{tmp0:03X},A"));
        for i in 1..4 {
            self.emit(format!("    MOVF 0x{:03X},W,A", pa + i));
            self.emit(format!("    XORWF 0x{:03X},W,A", pb + i));
            self.emit(format!("    IORWF 0x{tmp0:03X},W,A"));
            self.emit(format!("    MOVWF 0x{tmp0:03X},A"));
        }
        // the 4-byte unsigned compare chain: C = (pa >= pb)
        self.emit(format!("    MOVF 0x{:03X},W,A", pb));
        self.emit(format!("    SUBWF 0x{:03X},W,A", pa));
        for i in 1..4 {
            self.emit(format!("    MOVF 0x{:03X},W,A", pb + i));
            self.emit("    BTFSS 0xFD8,0,A".to_string());
            self.emit(format!("    INCFSZ 0x{:03X},W,A", pb + i));
            self.emit(format!("    SUBWF 0x{:03X},W,A", pa + i));
        }
        // equal -> 0; pa < pb -> (negative ? 2 : 1); pa > pb -> (negative ? 1 : 2)
        self.emit(format!("    MOVF 0x{tmp0:03X},W,A"));
        self.emit("    BTFSC 0xFD8,2,A".to_string());
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit("    BTFSS 0xFD8,0,A".to_string());
        self.emit(format!("    GOTO {l_mag_lt}"));
        self.emit(format!("    BTFSS 0x{tmp1:03X}, 7,A"));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_mag_lt}:"));
        self.emit(format!("    BTFSS 0x{tmp1:03X}, 7,A"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("{l_sign_diff}:"));
        self.emit(format!("    BTFSS 0x{:03X}, 7,A", pa + 3));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_ret0}:"));
        self.emit(format!("    CLRF 0x{r:03X},A"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret1}:"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{r:03X},A"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret2}:"));
        self.emit("    MOVLW 0x02".to_string());
        self.emit(format!("    MOVWF 0x{r:03X},A"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret3}:"));
        self.emit("    MOVLW 0x03".to_string());
        self.emit(format!("    MOVWF 0x{r:03X},A"));
        self.emit("    RETURN".to_string());
    }

    /// The recipe body for one of the mul/div/rem/shift runtime routines,
    /// adapted from PIC14's machine-verified epicurus asm but rewritten for
    /// PIC18: the muls use hardware `MULWF` (8x8 -> PRODH:PRODL, schoolbook
    /// partials, no shift-add loop), the divmod/shift loops use real branch
    /// instructions instead of skip-sensitive idioms (PIC18 branches are
    /// absolute, so a MOVLB between a test and its target is harmless, and
    /// the whole routine frame has NO single-GPR-bank constraint), and
    /// `RLCF`/`RRCF` replace `RLF`/`RRF`.
    ///
    /// Args arrive in the routine's `{func}::{param}` slots; the result
    /// goes to the fixed retval slots; working state lives in
    /// `{func}::__scr`. An `_isr` copy of a routine reads ITS OWN slots
    /// (the `cur_func` map) so the ISR frame never overlaps the main
    /// frame.
    fn emit_routine(&mut self) {
        let name = self.cur_func;
        let scr = self.slot_addr(name, "__scr").direct();
        let recipe = name.strip_suffix("_isr").unwrap_or(name);
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
                for addr in [pa, pa + 3, pb, pb + 3, scr, scr + 13] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                self.emit_f32_extract(pa, scr, scr + 1, scr + 2, false);
                self.emit_f32_extract(pb, scr + 5, scr + 6, scr + 7, recipe == "__sub_f32");
                self.emit_f32_add_body(scr);
            }
            "__mul_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                for addr in [a, a + 3, b, b + 3, scr, scr + 13] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                self.emit_f32_mul_body(a, b, scr);
            }
            "__div_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                for addr in [a, a + 3, b, b + 3, scr, scr + 11] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                self.emit_f32_div_body(a, b, scr);
            }
            "__uitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                for addr in [val, val + 3, scr, scr + 7] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                self.emit_f32_uitofp_body(val, scr, None);
            }
            "__sitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                for addr in [val, val + 3, scr, scr + 7] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                let sign = scr + 5;
                let l_pos = self.fresh_label();
                self.emit(format!("    MOVF 0x{:03X},W,A", val + 3));
                self.emit("    ANDLW 0x80".to_string());
                self.emit(format!("    MOVWF 0x{sign:03X},A"));
                self.emit(format!("    BTFSS 0x{:03X},7,A", val + 3));
                self.emit(format!("    GOTO {l_pos}"));
                self.neg_in_place(val, 4);
                self.emit(format!("{l_pos}:"));
                self.emit_f32_uitofp_body(val, scr, Some(sign));
            }
            "__fptoui_f32" | "__fptosi_f32" => {
                let val = self.slot_addr(name, "val").direct();
                for addr in [val, val + 3, scr, scr + 7] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo + 3 <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo + 3,
                    self.access_bank_hi
                );
                self.emit_f32_fptoi_body(val, scr, recipe == "__fptosi_f32");
            }
            "__cmp_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                for addr in [a, a + 3, b, b + 3, scr, scr + 5] {
                    assert!(
                        addr <= self.access_bank_hi,
                        "float routine {name} frame exceeds the access-bank GPR region (0x{addr:03X} > 0x{:03X})",
                        self.access_bank_hi
                    );
                }
                assert!(
                    self.retval_lo <= self.access_bank_hi,
                    "float routine {name} retval exceeds the access-bank GPR region (0x{:03X} > 0x{:03X})",
                    self.retval_lo,
                    self.access_bank_hi
                );
                self.emit_f32_cmp_body(a, b, scr);
            }
            other => panic!("isel-pic18: no recipe for runtime routine {other}"),
        }
    }
}

/// The classic iterative dominator sets for a function's CFG: `doms[b]` is
/// the set of blocks that dominate block `b`. Used to classify phi-copy
/// edges: `pred -> merge` is a BACK edge iff `merge` dominates `pred`  -  the
/// pred is inside the merge's loop, so on that edge the merge's phi slots
/// hold the CURRENT iteration's values. Ported from `isel`'s own
/// `block_dominators` (`crates/isel/src/lib.rs:3977-4022`)  -  that function
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
/// private to that crate  -  same reason as `block_dominators` above).
///
/// The ordering depends on whether the edge is a BACK edge into the merge
/// (`back_edge`, from `block_dominators`  -  the merge dominates the pred):
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

pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    let (common_lo, _) = device
        .fixed_retval
        .expect("isel-pic18's fixed retval region needs a fixed_retval reservation");
    let (_, access_bank_hi) = device
        .access_bank
        .expect("isel-pic18's access-bank frame checks need an access_bank reservation");
    let mut out: Vec<String> = Vec::new();
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
    if !m.module_asm.is_empty() {
        out.push("; module asm".to_string());
        for entry in &m.module_asm {
            for line in entry.split('\n') {
                out.push(line.to_string());
            }
        }
    }
    // Shared across every `Gen` below so `fresh_label()` never repeats a
    // `tmp{n}:` label across two different functions in the same output.
    let mut tmp = 0u32;
    // Every pointer reg in the module, folded once up front; later tasks'
    // pointer emitters consume it via `Gen::resolved_for`.
    let resolved = resolve_pointers(m);
    // P5: at most one ISR (single-vector compatibility mode, the docs/29
    // ruling this plan settles). The vector entry is the ISR body itself:
    // the hardware jumps to 0x0008 (GIE-gated), so the ISR must be emitted
    // FIRST at that address, before any ordinary function.
    let isr_count = m.funcs.iter().filter(|f| f.isr).count();
    assert!(
        isr_count <= 1,
        "isel-pic18: {isr_count} interrupt handlers, single-vector compatibility mode supports at most 1 (multiple ISRs not yet supported; see the P5 plan ruling)"
    );
    let mut funcs: Vec<&Func> = m.funcs.iter().collect();
    funcs.sort_by_key(|f| !f.isr); // ISR first, then ordinary functions
    for f in funcs {
        // P6: a runtime routine (or its `_isr` copy) emits its recipe body
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
                cur_func: &f.name,
                isr: f.isr,
                tmp: &mut tmp,
                out: Vec::new(),
            };
            g.emit_routine();
            out.extend(g.out);
            continue;
        }
        // CC-4 naked: verbatim, no prologue, panic on non-Asm, barrier markers.
        if f.naked {
            out.push(format!("{}:", f.name));
            out.push("; --- asm start ---".to_string());
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
            out.push("".to_string());
            continue;
        }
        if f.isr {
            // The vector entry at 0x0008 IS the ISR body (the hardware
            // jumps there with GIE cleared; no GOTO indirection, matching
            // PIC14's vector-as-entry convention). `__start`'s reset GOTO
            // at 0x0000 reaches it regardless: PIC18 GOTO/CALL are absolute
            // 20-bit.
            out.push("    org 0x0008".to_string());
        }
        let mut g = Gen {
            m,
            addrs,
            resolved: &resolved,
            retval_lo: common_lo,
            access_bank_hi,
            bsr: None,
            cur_func: &f.name,
            isr: f.isr,
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
            let lbl = if i == 0 {
                f.name.clone()
            } else {
                format!("{}_L{}", f.name, b.label)
            };
            labels.insert(b.label.clone(), lbl);
        }
        // Phi elimination: for each (predecessor, merge) EDGE  -  not just
        // the predecessor  -  the copies that must run when that edge is
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
        for (bi, b) in f.blocks.iter().enumerate() {
            g.emit_label(&labels[&b.label]);
            if bi == 0 && f.isr {
                // The ISR save prologue, right after the vector entry at
                // 0x0008. The preempted main's live state this saves:
                //   - the in-flight return value (0x0000-0x0003): an ISR
                //     that itself calls a value-returning function would
                //     clobber it (PIC14 M13's identical hazard)
                //   - STATUS/BSR/FSR0L/FSR0H: the ISR body's own banked
                //     access and FSR0 pointer work would clobber them
                //   - TBLPTRU/H/L: a const read is a multi-instruction
                //     setup, so an interrupt taken mid-setup leaves a torn
                //     pointer the ISR body's own const reads would misread
                // W is saved LAST via MOVWF (which clobbers nothing), so
                // the preempted main's W is intact until the very last
                // save instruction.
                for (src, dst) in [
                    (common_lo, common_lo + 12),
                    (common_lo + 1, common_lo + 13),
                    (common_lo + 2, common_lo + 14),
                    (common_lo + 3, common_lo + 15),
                    (0xFD8, common_lo + 1), // STATUS
                    (0xFE0, common_lo + 2), // BSR
                    (0xFE9, common_lo + 3), // FSR0L
                    (0xFEA, common_lo + 4), // FSR0H
                    (0xFF6, common_lo + 5), // TBLPTRL
                    (0xFF7, common_lo + 6), // TBLPTRH
                    (0xFF8, common_lo + 7), // TBLPTRU
                ] {
                    g.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
                }
                g.emit("    MOVWF 0x0004,A".to_string()); // W, last
            }
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
                    // are real branches to a label  -  so each edge's copies
                    // can be inlined directly along that edge's own path,
                    // with no intermediate copy-block indirection needed.
                    match (t_copies, f_copies) {
                        // Plain branch, no phi consumers on either edge
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
                Some(Inst::Ret(None)) if g.isr => {
                    // The ISR restore epilogue replaces `ret`. MOVFF-based
                    // (never touches STATUS), so the interrupted main's
                    // Z/N come back intact; only the final W restore via
                    // MOVF sets Z/N from the moved value (the one accepted
                    // flag loss, same as PIC14's W-last convention). The
                    // retval snapshot is restored first, then the SFRs
                    // (reverse of the prologue), STATUS, and W last.
                    for (src, dst) in [
                        (common_lo + 15, common_lo + 3),
                        (common_lo + 14, common_lo + 2),
                        (common_lo + 13, common_lo + 1),
                        (common_lo + 12, common_lo),
                        (common_lo + 7, 0xFF8), // TBLPTRU
                        (common_lo + 6, 0xFF7), // TBLPTRH
                        (common_lo + 5, 0xFF6), // TBLPTRL
                        (common_lo + 4, 0xFEA), // FSR0H
                        (common_lo + 3, 0xFE9), // FSR0L
                        (common_lo + 2, 0xFE0), // BSR
                        (common_lo + 1, 0xFD8), // STATUS
                    ] {
                        g.emit(format!("    MOVFF 0x{src:03X}, 0x{dst:03X}"));
                    }
                    g.emit("    MOVF 0x0004, W, A".to_string()); // W last
                    g.emit("    RETFIE".to_string());
                }
                Some(Inst::Ret(Some(_))) if g.isr => {
                    panic!(
                        "isel-pic18: interrupt handler @{} must be void (cannot return a value)",
                        f.name
                    )
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
    // Const string literals copied to RAM need init before main.
    {
        let mut init: Vec<String> = Vec::new();
        for g in &m.globals {
            if g.is_const && addrs.contains_key(&g.name) {
                let base = addrs[&g.name];
                for (i, b) in g.bytes.iter().enumerate() {
                    let addr = base + i as u16;
                    // A function-address field (epic-cc#154) materializes
                    // the link-time label literal.
                    if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                        let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                        init.push(format!("    MOVLW {lit}({f})"));
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
        out.extend(init);
        out.push("    call main".to_string());
        out.push("    sleep".to_string());
    }
    // P4: every `const` (flash) global becomes a `DB` table after the code,
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
        // Chunk the plain bytes 8 per line (the pre-#154 layout); a
        // function-address field (epic-cc#154) materializes the link-time
        // label literal, byte 0 = LOW(fn), byte 1 = HIGH(fn), resolved by
        // the assembler's symbol table, and splits its own line.
        let mut chunk: Vec<String> = Vec::new();
        for (i, b) in g.bytes.iter().enumerate() {
            if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                if !chunk.is_empty() {
                    out.push(format!("    db {}", chunk.join(", ")));
                    chunk.clear();
                }
                let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                out.push(format!("    db {lit}({f})"));
            } else {
                chunk.push(format!("0x{b:02X}"));
                if chunk.len() == 8 {
                    out.push(format!("    db {}", chunk.join(", ")));
                    chunk.clear();
                }
            }
        }
        if !chunk.is_empty() {
            out.push(format!("    db {}", chunk.join(", ")));
        }
        out.push("".to_string());
    }
    // gpasm requires the `end` directive (our own assembler tolerates its
    // absence); PIC14's `isel::select` emits it the same way.
    out.push("    end".to_string());
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the `tmp` field's shared-borrow requirement:
    /// `Gen::tmp` must be `&'m mut u32` backed by one counter that outlives
    /// every function's `Gen`, not an owned `u32` reset per function  -  an
    /// owned counter would let two functions' `fresh_label()` calls both
    /// emit `tmp0`, a duplicate label that fails to assemble once Task 12
    /// starts calling `fresh_label()` for real. `fresh_label()` itself has
    /// no caller yet in Task 3's scope (Select/Icmp/Br land later), so this
    /// constructs two `Gen`s directly  -  the way `select()` constructs one
    /// per function  -  sharing one backing `tmp: &mut u32`, exactly as
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
                access_bank_hi: 0x5F,
                bsr: None,
                cur_func: "f",
                isr: false,
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
                access_bank_hi: 0x5F,
                bsr: None,
                cur_func: "f",
                isr: false,
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
            access_bank_hi: 0x5F,
            bsr: None,
            cur_func: "main",
            isr: false,
            tmp,
            out: Vec::new(),
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
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
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
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        assert_eq!(g.operand(0x0090), (1, 0x90));
        assert!(
            g.out.iter().any(|l| l.contains("MOVLB")),
            "the banked range needs a MOVLB"
        );
    }

    #[test]
    fn sfr_high_segment_needs_no_movlb() {
        let m = Module {
            globals: Vec::new(),
            funcs: Vec::new(),
            module_asm: Vec::new(),
        };
        let addrs = HashMap::new();
        let resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
        let mut tmp = 0u32;
        let mut g = gen(&m, &addrs, &resolved, &mut tmp);
        // FSR0L, the address this task exists to fix.
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
}
