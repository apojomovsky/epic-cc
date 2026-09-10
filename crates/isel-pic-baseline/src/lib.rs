//! `isel-pic-baseline`: instruction selection for the PIC12F509 baseline core.
//!
//! A fork of classic `isel`'s integer spine (i8/i16 loads/stores, add/sub/
//! and/or/xor, zext/sext/trunc, all ten icmp predicates, const-count shifts
//! via RLF/RRF, calls, returns, selects, phis, const-length memcpys, inline
//! asm), adapted for the 12-bit baseline ISA and the D-2 bank-discipline
//! (docs/37): every direct access to a banked GPR is preceded by an
//! unconditional `BCF`/`BSF FSR,<bank-bit>` reassert, and every `INDF` touch
//! re-loads FSR from the pointer's source immediately before the touch.
//!
//! Baseline ISA deltas from classic (all load-bearing):
//! - No `ADDLW`: `W = W + k` stages W in the fixed `tmp` byte
//!   (`MOVWF tmp; MOVLW k; ADDWF tmp,W`); carry folds use
//!   `MOVWF tmp; BTFSC/BTFSS STATUS,0; INCF tmp,F; MOVF tmp,W`.
//! - No `SUBLW`: a const-LHS sub/compare stages the const in `tmp`
//!   (`MOVLW k; MOVWF tmp; MOVF a,W; SUBWF tmp,W`); `SUBWF` itself is
//!   baseline-legal and stays for the reg shapes.
//! - No `RETURN`: every return is `RETLW 0` (void) or retval-in-fixed-slots
//!   plus `RETLW 0` (RETLW always clobbers W with its literal).
//! - No `PCLATH`: single pass, no page assignment; every `CALL` targets the
//!   low 256 words of page 0 (CALL forces PC bit 8 to 0), so a program past
//!   0x100 words panics (multi-page placement is D-5/P4).
//! - The 6-bit flat FSR: addresses fit 0x00-0x3F, pointer-value hi bytes are
//!   0, and there is no IRP/linear alias.
//!
//! Skip-pair rule (load-bearing): never emit a reassert between a skip op
//! (BTFSC/BTFSS/INCFSZ/DECFSZ) and its target. Compare folds stage the fold
//! operand in common RAM (`tmp2`) and emit the banked operand's reassert
//! BEFORE the BTFSS/BTFSC test.
//!
//! Landed later: P4 (const RETLW tables, variadic functions, dynamic
//! memcpy), P6 (i32 arithmetic, mul/div/rem, variable shifts, runtime
//! routines), P7 (float). Each panics with its landing phase.

use device::Device;
use ir::{BinOp, Inst, MemLen, Module, SrcLoc, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};
use std::collections::{HashMap, HashSet};

/// The byte address of a literal-pointer operand (`"0x<K>"`, the
/// `inttoptr (<ty> <k> to ptr)` constant-pointer form parsed by irparse).
fn literal_ptr_addr(ptr: &str) -> u16 {
    let lit = ptr
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("isel: malformed literal pointer {ptr:?}"));
    u16::from_str_radix(lit, 16)
        .unwrap_or_else(|_| panic!("isel: malformed literal pointer {ptr:?}"))
}

/// The D-2 reassert lines for one direct access to `addr`: one
/// `BCF`/`BSF FSR,<5+i>` per bank bit, or nothing for a bank-independent
/// (common-RAM/SFR) address. Free function for the `__start` const-init
/// loop, which is built as plain strings rather than through a Gen.
fn bank_select_lines(device: &Device, addr: u16) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(bank) = device.bank_of(addr) {
        for i in 0..device.fsr_bank_bits {
            let bit = 5 + i;
            let op = if bank & (1 << i) != 0 { "BSF" } else { "BCF" };
            lines.push(format!("    {op} FSR, {bit}"));
        }
    }
    lines
}
/// The 5-bit file-register operand for a direct access to `addr`: a banked
/// GPR emits as its offset within its bank (`addr & 0x1F`, the D-2 reassert
/// selects the bank via FSR<5>); common/SFR addresses pass through. Free
/// function for the `__start` const-init loop (no Gen there); the method
/// `Gen::file_reg` delegates to it.
fn file_reg(device: &Device, addr: u16) -> u8 {
    if device.bank_of(addr).is_some() {
        (addr & 0x1F) as u8
    } else {
        assert!(
            addr <= 0x1F,
            "isel: file register 0x{addr:02X} out of 5-bit range"
        );
        addr as u8
    }
}

/// How a single-byte pointer access completes after `emit_ptr_setup`.
enum Addr {
    /// A plain file register (the address is statically known).
    Direct(u16),
    /// FSR is set up; the access goes through INDF.
    Indirect,
}

/// Per-function codegen state. All addresses come from the module-wide map;
/// `cur_func` selects the current function's local entries. The fixed
/// staging bytes live in common RAM (bank-independent): `scratch` 0x07,
/// `retval_lo` 0x08 (four bytes), `tmp` 0x0C (ADDLW-replacement staging),
/// `tmp2` 0x0D (compare-fold staging).
struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    device: &'m Device,
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)`: GEP chains fully collapsed (base
    /// `Reg` replaced by the base's own entry), plus the seeded pointer
    /// bases (byval/sret params and allocas). `gep`/`alloca` themselves
    /// emit nothing; each `load`/`store`/`memcpy` through a pointer reg
    /// lowers the pointer at its use.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    scratch: u16,
    retval_lo: u16,
    tmp: u16,
    tmp2: u16,
    cur_func: &'m str,
    /// Module-scoped fresh-label counter, shared across every function so the
    /// emitted `tmp{n}:` labels stay unique in the single `.asm` output.
    label_counter: &'m mut u32,
    /// The slot address whose value `emit_w_store`/`emit_w_load` last left
    /// in W, or `None` when unknown. `emit_bank_select` (BCF/BSF FSR)
    /// preserves it: the reassert touches neither W nor STATUS. Plain
    /// `emit` clears it, so a stale belief survives only across adjacent
    /// `emit_w_*` calls within one function.
    w_holds: Option<u16>,
    /// The source location of the instruction currently being emitted, or
    /// `None` for compiler-generated glue. `emit` records it on the line it
    /// pushes, so the parallel `locs` vector stays index-aligned with `out`.
    cur_loc: Option<SrcLoc>,
    out: Vec<String>,
    /// One source location per emitted line, index-aligned with `out`.
    locs: Vec<Option<SrcLoc>>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.w_holds = None;
        self.out.push(s.into());
        self.locs.push(self.cur_loc.clone());
    }

    /// The D-2 reassert: an unconditional `BCF`/`BSF FSR,<bank-bit>` per
    /// bank bit of `addr`'s bank, immediately before a direct access to a
    /// banked GPR. Pushes directly to `out`/`locs` (NOT via `emit`): the
    /// reassert touches neither W nor STATUS, so the `w_holds` cache stays
    /// valid across it. Call at the top of `emit_w_load`/`emit_w_store`
    /// BEFORE the cache check: the reassert must happen even when the
    /// MOVF/MOVWF is skipped.
    fn emit_bank_select(&mut self, addr: u16) {
        let cur = self.cur_loc.clone();
        for line in bank_select_lines(self.device, addr) {
            self.out.push(line);
            self.locs.push(cur.clone());
        }
    }

    /// The 5-bit file-register operand for a direct access to `addr`. A
    /// banked GPR emits as its offset within its bank (`addr & 0x1F`); the
    /// D-2 reassert selects the bank via FSR<5>. Common/SFR addresses pass
    /// through (they already fit the 5-bit field). The mirror gap panics
    /// via `bank_of` rather than silently aliasing.
    fn file_reg(&self, addr: u16) -> u8 {
        file_reg(self.device, addr)
    }

    /// `MOVWF addr`, unless `addr` is already known to hold W's value from
    /// an immediately preceding `emit_w_store`/`emit_w_load` of the same
    /// address. The bank reassert always runs (the skipped MOVWF's bank
    /// context is what FSR must hold). Marks `addr` either way.
    fn emit_w_store(&mut self, addr: u16) {
        self.emit_bank_select(addr);
        if self.w_holds != Some(addr) {
            self.emit(format!("    MOVWF 0x{:02X}", self.file_reg(addr)));
        }
        self.w_holds = Some(addr);
    }

    /// `MOVF addr, W`, unless `addr`'s value is already known to be in W.
    /// The bank reassert always runs. Marks `addr` either way.
    fn emit_w_load(&mut self, addr: u16) {
        self.emit_bank_select(addr);
        if self.w_holds != Some(addr) {
            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(addr)));
        }
        self.w_holds = Some(addr);
    }
    /// reassert is needed, and the sequence is self-contained (tmp is dead
    /// between instructions), so it is free at any use.
    fn emit_add_w_lit(&mut self, k: u8) {
        self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
        self.emit(format!("    MOVLW 0x{k:02X}"));
        self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
    }

    /// True when `name` is a plain pointer param of the current function,
    /// whose slot holds a runtime address rather than being the object
    /// itself.
    fn param_holds_addr(&self, name: &str) -> bool {
        self.m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .map(|f| f.params.iter().any(|p| p.name == name && p.ptr))
            .unwrap_or(false)
    }

    fn slot_addr(&self, func: &str, name: &str) -> Slot {
        Slot::Direct(
            *self
                .addrs
                .get(&ssa_key(func, name))
                .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}")),
        )
    }

    /// Substitute `$0`/`%0` placeholders in an inline asm template with the
    /// allocated address of each `*m` operand. Each operand's `ptr` is either
    /// `@global` or `%local`; globals are looked up directly, locals via
    /// `ssa_key(func, name)`. GEP-derived pointers panic (docs/31 §3).
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
                                .unwrap_or_else(|| panic!("isel: no address for @{g}"))
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

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> Slot {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
            Val::Global(g) => Slot::Direct(
                *self
                    .addrs
                    .get(g)
                    .unwrap_or_else(|| panic!("isel: no address for @{g}")),
            ),
            Val::Const(k) => Slot::Direct((*k & 0xFF) as u16),
        }
    }

    /// The byte address of a RAM global (from the map).
    fn global_addr(&self, name: &str) -> u16 {
        *self
            .addrs
            .get(name)
            .unwrap_or_else(|| panic!("isel: no address for @{name}"))
    }

    /// Whether `name` is a const (flash) global: read via RETLW tables in
    /// P4. A const that was copied to RAM (alloc placed it in `addrs`
    /// because it is used as a pointer call argument) is treated as RAM.
    fn global_is_const(&self, name: &str) -> bool {
        if self.addrs.contains_key(name) {
            return false;
        }
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .is_const
    }

    /// Whether `name` is a function (a valid indirect-call target) rather
    /// than a RAM/const global. A function's address is a link-time label
    /// literal, materialized as a LOW byte plus a 0 hi (6-bit data space).
    fn is_function(&self, name: &str) -> bool {
        self.m.funcs.iter().any(|f| f.name == name)
    }

    /// The byte size of a global.
    fn global_size(&self, name: &str) -> u16 {
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .size
    }

    /// The resolved `(base, k, terms)` for a pointer reg `%r`. Anything
    /// else is a missing pointer and panics.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
    }
    /// How a byte access at `ptr + byte_off` completes: `Direct(a)` reads or
    /// writes the plain file register `a` (the caller reasserts the bank);
    /// `Indirect` means FSR was just re-loaded and the access goes through
    /// INDF. A const (flash) base is rejected (loads take the P4 table path;
    /// stores panic). Every materialized address is asserted 6-bit.
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => {
                assert!(
                    !self.global_is_const(g),
                    "isel: store to const (flash) global @{g}"
                );
                let a = self.global_addr(g) + u16::from(byte_off);
                assert!(
                    a <= 0x3F,
                    "isel: direct address 0x{a:02X} exceeds the 6-bit data space"
                );
                Addr::Direct(a)
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        assert!(
                            !self.global_is_const(name),
                            "isel: store to const (flash) global @{name}"
                        );
                        if terms.is_empty() {
                            let a = self.global_addr(name) + u16::from(k) + u16::from(byte_off);
                            assert!(
                                a <= 0x3F,
                                "isel: direct address 0x{a:02X} exceeds the 6-bit data space"
                            );
                            Addr::Direct(a)
                        } else {
                            self.emit_fsr_to(self.global_addr(name), k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        if *indirect || self.param_holds_addr(sname) {
                            self.emit_fsr_indirect(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else if terms.is_empty() {
                            let a = sa + u16::from(k) + u16::from(byte_off);
                            assert!(
                                a <= 0x3F,
                                "isel: direct address 0x{a:02X} exceeds the 6-bit data space"
                            );
                            Addr::Direct(a)
                        } else {
                            self.emit_fsr_to(sa, k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// `W = RAM[ptr + byte_off]`. Direct bases read the plain file register
    /// (with a reassert); dynamic bases reload FSR immediately before the
    /// INDF read. A const (flash) base panics (the P4 RETLW path).
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), _, _) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        panic!("isel: const (flash) table reads land in P4 (docs/37)");
                    }
                }
            }
            Val::Global(g) => {
                if self.global_is_const(g) {
                    panic!("isel: const (flash) table reads land in P4 (docs/37)");
                }
            }
            Val::Const(_) => panic!("isel: load through a constant pointer"),
        }
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(a)));
            }
            Addr::Indirect => self.emit("    MOVF INDF, W".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = W`.
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF 0x{:02X}", self.file_reg(a)));
            }
            Addr::Indirect => self.emit("    MOVWF INDF".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`. The address setup comes
    /// first (its FSR/scratch computation clobbers W), so the value loads
    /// only after FSR is final.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_load_byte(val, byte_off);
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF 0x{:02X}", self.file_reg(a)));
            }
            Addr::Indirect => {
                self.emit_load_byte(val, byte_off);
                self.emit("    MOVWF INDF".to_string());
            }
        }
    }

    /// `FSR = base_addr + k + byte_off + Σ scale×%reg`, for the flat 6-bit
    /// FSR: the materialized address must fit 0x00-0x3F. This IS the
    /// D-2 reassertion for indirect accesses: FSR<5> is set atomically as
    /// part of the flat address, so no separate bank reassert is needed.
    /// A single scale-1 term stages through the fixed `tmp` byte (no
    /// ADDLW); general sums accumulate in `scratch` first, then the
    /// `MOVLW lit; ADDWF scratch,W` shape adds the literal without a temp.
    fn emit_fsr_to(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let sum = base_addr + u16::from(k) + u16::from(byte_off);
        assert!(
            sum <= 0x3F,
            "isel: FSR base 0x{base_addr:02X} + k {k} + off {byte_off} exceeds the 6-bit data space"
        );
        let lit = (sum & 0x3F) as u8;
        match terms {
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(a)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit(format!("    MOVLW 0x{lit:02X}"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
                self.emit("    MOVWF FSR".to_string());
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit(format!("    MOVLW 0x{lit:02X}"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit("    MOVWF FSR".to_string());
            }
        }
    }

    /// Indirect (sret/pointer-value) FSR setup: `FSR = [slot] + k +
    /// byte_off + Σ terms`. The slot holds the target address's lo byte (a
    /// 6-bit address; the hi byte of a pointer value is 0). No ADDLW on the
    /// baseline, so the static `k + off` adds through `tmp`.
    fn emit_fsr_indirect(&mut self, slot_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0x3F,
            "isel: indirect offset k {k} + off {byte_off} out of 6-bit range"
        );
        let kk = kk as u8;
        if terms.is_empty() {
            self.emit_bank_select(slot_addr);
            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(slot_addr)));
            self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
            self.emit(format!("    MOVLW 0x{kk:02X}"));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
            self.emit("    MOVWF FSR".to_string());
        } else {
            self.emit_accum_terms(terms);
            self.emit_bank_select(slot_addr);
            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(slot_addr)));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
            self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
            self.emit(format!("    MOVLW 0x{kk:02X}"));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
            self.emit("    MOVWF FSR".to_string());
        }
    }

    /// `scratch = Σ scale×%reg`: W = 0, then per term
    /// `MOVF %r,W; ADDWF scratch,W; MOVWF scratch` repeated `scale` times.
    /// ADDWF f,W computes W = f + W, so W holds %r only until the first
    /// ADDWF: it MUST be reloaded before each repetition. `scratch` is
    /// common RAM, so no reassert is needed for it; each term's slot gets
    /// one (a reassert between the MOVF and the ADDWF is safe: MOVF is not
    /// a skip op).
    fn emit_accum_terms(&mut self, terms: &[(u8, String)]) {
        self.emit("    MOVLW 0x00".to_string());
        self.emit_w_store(self.scratch);
        for (scale, r) in terms {
            let a = self.val_addr(&Val::Reg(r.clone())).direct();
            for _ in 0..*scale {
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(a)));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit_w_store(self.scratch);
            }
        }
    }

    /// W = byte `idx` of `val`. Pointer-typed regs resolve to their folded
    /// GEP `(base, k, terms)`: the address VALUE's lo byte is the
    /// materialized address; the hi byte is 0 on the baseline (6-bit data
    /// space), emitted as `MOVLW 0x00` without reading the slot's hi byte.
    fn emit_load_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                if let Some((base, k, terms)) =
                    self.resolved.get(&ssa_key(self.cur_func, r)).cloned()
                {
                    // Byte 1 of a 6-bit address value is 0 (no carry can
                    // escape byte 0: the sum is asserted 6-bit at every
                    // materialization).
                    if idx == 1 {
                        self.emit("    MOVLW 0x00".to_string());
                        return;
                    }
                    assert!(
                        k == 0 || terms.is_empty(),
                        "isel: GEP with both a constant offset and dynamic terms \
                         loses the term's carry; not supported"
                    );
                    if let Base::Global(name) = &base {
                        let addr = self.global_addr(name).wrapping_add(k as u16);
                        let lo = (addr & 0x3F) as u8;
                        match terms.as_slice() {
                            [] => {
                                self.emit(format!("    MOVLW 0x{lo:02X}"));
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                self.emit(format!("    MOVLW 0x{lo:02X}"));
                                self.emit_bank_select(ra);
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra)));
                            }
                            _ => {
                                assert!(
                                    terms.len() == 2 && terms.iter().all(|(sc, _)| *sc == 1),
                                    "isel: multi-term GEP load with {terms:?} not supported"
                                );
                                let ra1 = self.val_addr(&Val::Reg(terms[0].1.clone())).direct();
                                let ra2 = self.val_addr(&Val::Reg(terms[1].1.clone())).direct();
                                self.emit(format!("    MOVLW 0x{lo:02X}"));
                                self.emit_bank_select(ra1);
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra1)));
                                self.emit_bank_select(ra2);
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra2)));
                            }
                        }
                        return;
                    }
                    let sa = match &base {
                        Base::Slot(sname, indirect) => {
                            assert!(
                                *indirect || self.param_holds_addr(sname),
                                "isel: cannot take the value of a GEP over {base:?}"
                            );
                            self.slot_addr(self.cur_func, sname).direct()
                        }
                        other => panic!("isel: cannot take the value of a GEP over {other:?}"),
                    };
                    match terms.as_slice() {
                        [] => {
                            self.emit_bank_select(sa);
                            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(sa)));
                            if k != 0 {
                                self.emit_add_w_lit(k);
                            }
                        }
                        [(1, reg)] => {
                            let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                            self.emit_bank_select(sa);
                            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(sa)));
                            self.emit_bank_select(ra);
                            self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra)));
                        }
                        _ => {
                            assert!(
                                terms.len() == 2 && terms.iter().all(|(sc, _)| *sc == 1),
                                "isel: multi-term GEP load with {terms:?} not supported"
                            );
                            let ra1 = self.val_addr(&Val::Reg(terms[0].1.clone())).direct();
                            let ra2 = self.val_addr(&Val::Reg(terms[1].1.clone())).direct();
                            self.emit_bank_select(sa);
                            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(sa)));
                            self.emit_bank_select(ra1);
                            self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra1)));
                            self.emit_bank_select(ra2);
                            self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra2)));
                        }
                    }
                    return;
                }

                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_w_load(a + u16::from(idx));
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    // A function's address is a link-time label literal: the
                    // lo byte is LOW(g); the hi byte is 0 (the 0x100-word
                    // CALL ceiling keeps every function in the low half).
                    if idx == 0 {
                        self.emit(format!("    MOVLW LOW({g})"));
                    } else {
                        self.emit("    MOVLW 0x00".to_string());
                    }
                } else {
                    // A data global in value position is a pointer ADDRESS:
                    // materialize the lo byte; hi is 0 (6-bit data space).
                    let a = self.val_addr(&Val::Global(g.clone())).direct();
                    let b = if idx == 0 { (a & 0x3F) as u8 } else { 0 };
                    self.emit(format!("    MOVLW 0x{b:02X}"));
                }
            }
        }
    }

    /// W ^= byte `idx` of `val`.
    fn emit_xor_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    XORLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(a + u16::from(idx));
                self.emit(format!(
                    "    XORWF 0x{:02X}, W",
                    self.file_reg(a + u16::from(idx))
                ));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone())).direct();
                self.emit_bank_select(a + u16::from(idx));
                self.emit(format!(
                    "    XORWF 0x{:02X}, W",
                    self.file_reg(a + u16::from(idx))
                ));
            }
        }
    }

    /// Copy `val` (width `ty`) into the slot starting at `dst`.
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u16) {
        for i in 0..ty.bytes() {
            self.emit_load_byte(val, i);
            self.emit_w_store(dst + u16::from(i));
        }
    }

    /// Set the Z flag to (a == b) without disturbing other flags. The XORs
    /// of every byte pair accumulate in the fixed `scratch` byte, leaving Z
    /// set exactly when every byte was equal.
    fn emit_cmp_eq(&mut self, a: &Val, b: &Val, ty: Ty) {
        let n = ty.bytes();
        self.emit_load_byte(a, 0);
        self.emit_xor_byte(b, 0);
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        for i in 1..n {
            self.emit_load_byte(a, i);
            self.emit_xor_byte(b, i);
            self.emit(format!("    IORWF 0x{:02X}, W", self.scratch));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        }
    }

    /// W = byte `i` of `v`, with the sign bit complemented (XOR 0x80) when
    /// `signed` and `i` is the high (sign) byte.
    fn emit_load_cmp_byte(&mut self, v: &Val, i: u8, signed: bool, high: u8) {
        match v {
            Val::Const(k) => {
                let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                let b = if signed && i == high {
                    byte ^ 0x80
                } else {
                    byte
                };
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            _ => {
                let addr = self.val_addr(v).direct() + u16::from(i);
                if signed && i == high {
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit_bank_select(addr);
                    self.emit(format!("    XORWF 0x{:02X}, W", self.file_reg(addr)));
                } else {
                    self.emit_bank_select(addr);
                    self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(addr)));
                }
            }
        }
    }
    /// Set C = (a >= b), unsigned or signed. For i8 the SUBWF also leaves
    /// Z = (a == b); wider borrow chains leave only a byte-level Z, so
    /// equality appends `emit_cmp_eq` (C intact). A const RHS is the MOVLW
    /// subtrahend; a const LHS stages the const in `tmp` (no SUBLW):
    /// `SUBWF tmp,W` = `tmp - W`, so C = (k >= b).
    fn emit_cmp_c(&mut self, a: &Val, b: &Val, ty: Ty, signed: bool) {
        let n = ty.bytes();
        let high = n - 1;
        match (a, b) {
            (Val::Const(_), Val::Const(_)) => panic!("isel: constant folding not implemented"),
            (Val::Const(k), _) => {
                if n > 1 {
                    self.emit_cmp_c_const_lhs_wide(k, b, n, high, signed);
                    return;
                }
                // W = b0 (complemented when signed, from the load); stage it
                // in tmp2 and k0' in tmp: SUBWF tmp,W = k0' - b0'.
                self.emit_load_cmp_byte(b, 0, signed, high);
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp2));
                let k0 = (k & 0xFF) as u8;
                let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
                self.emit(format!("    MOVLW 0x{k0:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.tmp2));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.tmp));
            }
            _ => {
                if n > 1 {
                    self.emit_cmp_c_file_lhs_wide(a, b, n, high, signed);
                    return;
                }
                let aa = self.val_addr(a).direct();
                let use_scratch = signed;
                if use_scratch {
                    // Pre-store the complemented sign byte; MOVLW/XORWF/
                    // MOVWF do not touch C, and the SUBWF below sets it.
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit_bank_select(aa + high as u16);
                    self.emit(format!(
                        "    XORWF 0x{:02X}, W",
                        self.file_reg(aa + high as u16)
                    ));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                }
                self.emit_load_cmp_byte(b, 0, signed, high);
                let f = if use_scratch && n == 1 {
                    self.scratch
                } else {
                    aa
                };
                self.emit_bank_select(f);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(f)));
            }
        }
    }

    /// The multi-byte (n > 1, i16) borrow chain for `C = (a >= b)` with a
    /// file-LHS `a`, with the wrap-correct INCFSZ borrow folds. The skip
    /// pair's operands are banked, so the fold operand is staged in common
    /// RAM (`tmp2`) and the banked operand's reassert is emitted BEFORE the
    /// BTFSS test (never between the skip and its target). The signed high
    /// byte complements both sides into common RAM (`retval_lo`,
    /// `scratch`) first, where the classic shape needs only XORWF reasserts.
    fn emit_cmp_c_file_lhs_wide(&mut self, a: &Val, b: &Val, n: u8, high: u8, signed: bool) {
        let aa = self.val_addr(a).direct();
        // Byte 0 has no borrow-in; a single SUBWF leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit_bank_select(aa);
        self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa)));
        for i in 1..n {
            if signed && i == high {
                match b {
                    Val::Const(k) => {
                        let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                    }
                    _ => {
                        let addr = self.val_addr(b).direct() + u16::from(high);
                        self.emit("    MOVLW 0x80".to_string());
                        self.emit_bank_select(addr);
                        self.emit(format!("    XORWF 0x{:02X}, W", self.file_reg(addr)));
                    }
                }
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(aa + u16::from(high));
                self.emit(format!(
                    "    XORWF 0x{:02X}, W",
                    self.file_reg(aa + u16::from(high))
                ));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            } else {
                match b {
                    Val::Const(k) => {
                        // The const stages in common scratch; only the SUBWF
                        // file operand is banked, reasserted BEFORE the test.
                        let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                        self.emit_bank_select(aa + u16::from(i));
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    }
                    _ => {
                        let addr = self.val_addr(b).direct() + u16::from(i);
                        self.emit_bank_select(addr);
                        self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(addr)));
                        self.emit(format!("    MOVWF 0x{:02X}", self.tmp2));
                        self.emit_bank_select(aa + u16::from(i));
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{:02X}, W", self.tmp2));
                    }
                }
                self.emit(format!(
                    "    SUBWF 0x{:02X}, W",
                    self.file_reg(aa + u16::from(i))
                ));
            }
        }
    }

    /// The multi-byte (n > 1, i16) borrow chain for `C = (k >= b)` with a
    /// const LHS. Same staged folds as the file-LHS chain: the b byte in
    /// `tmp2`, the const byte in `tmp`, `SUBWF tmp,F` computing
    /// `k_i - (b_i + borrow)` in place. The extra `MOVF tmp2,W` reload is
    /// load-bearing: the `MOVWF tmp` staging clobbers W.
    fn emit_cmp_c_const_lhs_wide(&mut self, k: &i64, b: &Val, n: u8, high: u8, signed: bool) {
        // Byte 0 has no borrow-in; stage b0' in tmp2 and k0' in tmp.
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    MOVWF 0x{:02X}", self.tmp2));
        let k0 = (k & 0xFF) as u8;
        let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
        self.emit(format!("    MOVLW 0x{k0:02X}"));
        self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
        self.emit(format!("    MOVF 0x{:02X}, W", self.tmp2));
        self.emit(format!("    SUBWF 0x{:02X}, W", self.tmp));
        for i in 1..n {
            if signed && i == high {
                let addr = self.val_addr(b).direct() + u16::from(high);
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(addr);
                self.emit(format!("    XORWF 0x{:02X}, W", self.file_reg(addr)));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                self.emit(format!("    SUBWF 0x{:02X}, F", self.tmp));
            } else {
                let addr = self.val_addr(b).direct() + u16::from(i);
                self.emit_bank_select(addr);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(addr)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp2));
                let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.tmp2));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.tmp2));
                self.emit(format!("    SUBWF 0x{:02X}, F", self.tmp));
            }
        }
    }

    /// Materialize a flag predicate into `dst` as an i1. The final MOVWF is
    /// reasserted, but it sits after every skip pair, so no hazard.
    fn emit_materialize(&mut self, cond: &str, dst: u16) {
        let (skip, adj2) = match cond {
            "Z" => ("BTFSC STATUS, 2 ; Z", ""),
            "!Z" => ("BTFSS STATUS, 2 ; Z", ""),
            "C" => ("BTFSC STATUS, 0 ; C", ""),
            "!C" => ("BTFSS STATUS, 0 ; C", ""),
            "C&&!Z" => ("BTFSC STATUS, 0 ; C", "MOVLW 0x00"),
            "!C||Z" => ("BTFSS STATUS, 0 ; C", "MOVLW 0x01"),
            _ => panic!("isel: bad materialize cond {cond}"),
        };
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    {skip}"));
        self.emit("    MOVLW 0x01".to_string());
        if !adj2.is_empty() {
            self.emit("    BTFSC STATUS, 2 ; Z".to_string());
            self.emit(format!("    {adj2}"));
        }
        self.emit_bank_select(dst);
        self.emit(format!("    MOVWF 0x{:02X}", self.file_reg(dst)));
    }

    /// Branch on `cond`: Z = (cond == 0); if Z is set (cond == 0) go to `f`,
    /// otherwise (cond != 0) go to `t`. The reassert precedes the MOVF (the
    /// skip pair's target is a GOTO, not the reassert).
    fn emit_cond_branch(&mut self, cond: &Val, t: &str, f: &str) {
        match cond {
            Val::Reg(r) => {
                let ca = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(ca);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(ca)));
                self.emit("    BTFSC STATUS, 2 ; Z".to_string());
                self.emit(format!("    GOTO {f}"));
                self.emit(format!("    GOTO {t}"));
            }
            Val::Const(k) => {
                let l = if *k != 0 { t } else { f };
                self.emit(format!("    GOTO {l}"));
            }
            Val::Global(_) => panic!("isel: conditional branch on a global"),
        }
    }

    /// Whether pointer-select dst `name` was seeded by iselcore as an
    /// indirect slot: its bytes are a runtime address VALUE the select must
    /// materialize, not a folded pointer.
    fn select_is_seeded(&self, name: &str) -> bool {
        matches!(
            self.resolved.get(&ssa_key(self.cur_func, name)),
            Some((Base::Slot(_, true), 0, t)) if t.is_empty()
        )
    }

    /// Copy the address VALUE of `val` into the slot at `dst`: a `Const`
    /// literal writes the constant bytes, a `Global` writes its RAM address,
    /// a `Reg` reads the runtime address slot. The hi byte is 0 on the
    /// baseline (6-bit data space); the slot's hi byte is never read.
    fn emit_move_addr_to_slot(&mut self, val: &Val, dst: u16) {
        match val {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit_w_store(dst);
                self.emit("    MOVLW 0x00".to_string());
                self.emit_w_store(dst + 1);
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    self.emit(format!("    MOVLW LOW({g})"));
                    self.emit_w_store(dst);
                    self.emit("    MOVLW 0x00".to_string());
                    self.emit_w_store(dst + 1);
                } else {
                    let addr = self.global_addr(g);
                    assert!(
                        addr <= 0x3F,
                        "isel: global @{g} at 0x{addr:02X} exceeds the 6-bit data space"
                    );
                    self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                    self.emit_w_store(dst);
                    self.emit("    MOVLW 0x00".to_string());
                    self.emit_w_store(dst + 1);
                }
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                assert!(
                    k == 0 && terms.is_empty(),
                    "isel: cannot materialize a computed address ({base:?} k={k} terms={terms:?}) as a select arm"
                );
                let sa = match &base {
                    Base::Slot(sname, true) => self.slot_addr(self.cur_func, sname).direct(),
                    Base::Global(name) => {
                        let addr = self.global_addr(name);
                        assert!(
                            addr <= 0x3F,
                            "isel: global @{name} at 0x{addr:02X} exceeds the 6-bit data space"
                        );
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                        self.emit_w_store(dst);
                        self.emit("    MOVLW 0x00".to_string());
                        self.emit_w_store(dst + 1);
                        return;
                    }
                    other => panic!("isel: cannot materialize {other:?} as a select arm"),
                };
                self.emit_w_load(sa);
                self.emit_w_store(dst);
                self.emit("    MOVLW 0x00".to_string());
                self.emit_w_store(dst + 1);
            }
        }
    }

    /// `d = cond ? a : b` via an if/else jump over two copies. A pointer
    /// select seeded as an indirect slot materializes address bytes, not
    /// RAM contents.
    fn emit_select(&mut self, dst: &str, cond: &Val, ty: Ty, a: &Val, b: &Val) {
        let da = self.slot_addr(self.cur_func, dst).direct();
        let addr_value = self.select_is_seeded(dst);
        match cond {
            Val::Const(k) => {
                let v = if *k != 0 { a } else { b };
                if addr_value {
                    self.emit_move_addr_to_slot(v, da);
                } else {
                    self.emit_move_val_to_slot(v, ty, da);
                }
                return;
            }
            Val::Global(_) => panic!("isel: select condition is a global"),
            Val::Reg(_) => {}
        }
        let l_else = self.fresh_label();
        let l_end = self.fresh_label();
        let ca = match cond {
            Val::Reg(r) => self.val_addr(&Val::Reg(r.clone())).direct(),
            _ => unreachable!(),
        };
        self.emit_bank_select(ca);
        self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(ca)));
        self.emit("    BTFSC STATUS, 2 ; Z".to_string());
        self.emit(format!("    GOTO {l_else}"));
        if addr_value {
            self.emit_move_addr_to_slot(a, da);
        } else {
            self.emit_move_val_to_slot(a, ty, da);
        }
        self.emit(format!("    GOTO {l_end}"));
        self.emit(format!("{l_else}:"));
        if addr_value {
            self.emit_move_addr_to_slot(b, da);
        } else {
            self.emit_move_val_to_slot(b, ty, da);
        }
        self.emit(format!("{l_end}:"));
    }

    /// A fresh local label for intra-block jumps (select branches). The
    /// counter lives at module scope so labels are unique across functions.
    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.label_counter);
        *self.label_counter += 1;
        s
    }

    /// `d = a + b` for i16 (either operand may be a register; at most one a
    /// const). The high-byte carry folds through the fixed `tmp` byte (no
    /// ADDLW): `MOVWF tmp; BTFSC STATUS,0; INCF tmp,F`, then the second
    /// addend incorporates it.
    fn emit_add16(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add16 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_w_load(bb);
                self.emit_bank_select(ra);
                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra)));
                self.emit_w_store(dst);
                // High byte: W = bb+1 + carry, via the tmp fold. The
                // reassert between MOVF tmp,W and ADDWF is safe (MOVF is not
                // a skip op).
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(bb + 1)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCF 0x{:02X}, F", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.tmp));
                self.emit_bank_select(ra + 1);
                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                // W = ra + lo, via the tmp sequence (no ADDLW).
                self.emit_bank_select(ra);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(ra)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit(format!("    MOVLW 0x{lo:02X}"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
                self.emit_w_store(dst);
                // High byte: W = ra+1 + carry + hi. After the tmp fold,
                // MOVLW hi overwrites the stale W, so no reload is needed.
                self.emit_bank_select(ra + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(ra + 1)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCF 0x{:02X}, F", self.tmp));
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
                self.emit_w_store(dst + 1);
            }
            Val::Global(_) => panic!("isel: add16 with a global operand"),
        }
    }

    /// `d = a OP b` bytewise, for and/or/xor at i8 or i16. A const LHS is
    /// swapped to the RHS so the literal path is used, never reading a
    /// const as a file-register address.
    fn emit_commutative(&mut self, a: &Val, b: &Val, ty: Ty, dst: u16, op: &str, opw: &str) {
        let n = ty.bytes();
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: {op} needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                for i in 0..n {
                    self.emit_w_load(bb + u16::from(i));
                    self.emit_bank_select(ra + u16::from(i));
                    self.emit(format!("    {op} 0x{:02X}, W", ra + u16::from(i)));
                    self.emit_w_store(dst + u16::from(i));
                }
            }
            Val::Const(k) => {
                for i in 0..n {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit_w_load(ra + u16::from(i));
                    self.emit(format!("    {opw} 0x{byte:02X}"));
                    self.emit_w_store(dst + u16::from(i));
                }
            }
            Val::Global(_) => panic!("isel: {op} with a global operand"),
        }
    }

    /// `d = a - b` for i8: the subtrahend goes in W and SUBWF (baseline-
    /// legal) computes `f - W`, so `a` is the file operand. A const LHS is
    /// rejected by the caller.
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (*k & 0xFF) as u8));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa)));
                self.emit_w_store(dst);
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(bb)));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa)));
                self.emit_w_store(dst);
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = k - a` (const LHS) for `bytes`-wide values (1 or 2 in P2). Byte
    /// 0 stages k0 in `tmp`: `SUBWF tmp,W` = `k0 - a0` (no SUBLW). Higher
    /// bytes use the wrap-correct INCFSZ fold: `k_i` preloads into the dst,
    /// `a_i` copies to scratch, and `SUBWF` computes `k_i - (a_i + borrow)`
    /// in place. The dst's bank was reasserted by its own preload store, so
    /// no reassert sits inside the BTFSS/INCFSZ/SUBWF skip triple.
    fn emit_sub_const_lhs(&mut self, k: &i64, a: &Val, dst: u16, bytes: u8) {
        let aa = self.val_addr(a).direct();
        self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
        self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
        self.emit_bank_select(aa);
        self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(aa)));
        self.emit(format!("    SUBWF 0x{:02X}, W", self.tmp));
        self.emit_w_store(dst);
        for i in 1..bytes {
            let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
            self.emit_bank_select(aa + u16::from(i));
            self.emit(format!(
                "    MOVF 0x{:02X}, W",
                self.file_reg(aa + u16::from(i))
            ));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            self.emit(format!("    MOVLW 0x{kb:02X}"));
            self.emit_w_store(dst + u16::from(i));
            self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
            self.emit("    BTFSS STATUS, 0 ; C".to_string());
            self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
            self.emit(format!(
                "    SUBWF 0x{:02X}, F",
                self.file_reg(dst + u16::from(i))
            ));
        }
    }

    /// `d = a - b` for i16: low byte SUBWF, then the high byte with the
    /// borrow folded into a `tmp` copy of the subtrahend (the `MOVF tmp,W`
    /// reload is load-bearing: SUBWF reads W).
    fn emit_sub16(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{lo:02X}"));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa)));
                self.emit_w_store(dst);
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCF 0x{:02X}, F", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.tmp));
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(bb)));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa)));
                self.emit_w_store(dst);
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(bb + 1)));
                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCF 0x{:02X}, F", self.tmp));
                self.emit(format!("    MOVF 0x{:02X}, W", self.tmp));
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.file_reg(aa + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Global(_) => panic!("isel: sub16 with a global operand"),
        }
    }
    /// Copy each call arg into the callee's `{func}::{param}` slots. Pointer
    /// args materialize as a lo byte plus a 0 hi (6-bit data space); the
    /// extra (variadic) args past the named params panic (P4).
    fn emit_call_args(&mut self, func: &str, args: &[ir::CallArg]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        let named = callee.params.len();
        for (i, arg) in args.iter().enumerate() {
            if i >= named {
                panic!("isel: variadic functions land in P4 (docs/37)");
            }
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(func, pname).direct();
            if let Some(size) = arg.byval {
                assert_eq!(
                    size,
                    callee.params[i]
                        .byval
                        .expect("isel: byval arg for a non-byval param"),
                    "isel: byval size mismatch for arg {i} of @{func}"
                );
                for b in 0..size {
                    self.emit_ptr_load_byte(&arg.val, b);
                    self.emit_w_store(pa + u16::from(b));
                }
            } else if arg.sret {
                assert!(callee.params[i].sret, "isel: sret arg for a non-sret param");
                let addr = match &arg.val {
                    Val::Global(g) => self.global_addr(g),
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        assert!(
                            k == 0 && terms.is_empty(),
                            "isel: sret target must be a plain global or alloca slot (no offset)"
                        );
                        match &base {
                            Base::Global(name) => self.global_addr(name),
                            Base::Slot(sname, false) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                            Base::Slot(_, true) => {
                                panic!("isel: sret target cannot be an indirect (sret) slot")
                            }
                        }
                    }
                    Val::Const(_) => panic!("isel: sret target must be a global or an alloca slot"),
                };
                assert!(
                    addr <= 0x3F,
                    "isel: sret target 0x{addr:02X} exceeds the 6-bit data space"
                );
                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                self.emit_w_store(pa);
                self.emit("    MOVLW 0x00".to_string());
                self.emit_w_store(pa + 1);
            } else if arg.ty.is_none() {
                assert!(
                    !arg.sret && arg.byval.is_none(),
                    "isel: plain ptr arg must be non-sret/non-byval"
                );
                assert_eq!(
                    callee.params[i].width, 2,
                    "isel: callee ptr param must be 2 bytes"
                );
                match &arg.val {
                    Val::Global(g) => {
                        if self.is_function(g) {
                            self.emit(format!("    MOVLW LOW({g})"));
                            self.emit_w_store(pa);
                            self.emit("    MOVLW 0x00".to_string());
                            self.emit_w_store(pa + 1);
                        } else {
                            if self.global_is_const(g) {
                                let size = self.global_size(g);
                                panic!("isel: const global @{g} too large for RAM copy ({size} bytes, max 255)");
                            }
                            let addr = self.global_addr(g);
                            assert!(
                                addr <= 0x3F,
                                "isel: ptr arg @{g} at 0x{addr:02X} exceeds the 6-bit data space"
                            );
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                            self.emit_w_store(pa);
                            self.emit("    MOVLW 0x00".to_string());
                            self.emit_w_store(pa + 1);
                        }
                    }
                    Val::Const(c) => {
                        assert_eq!(*c, 0, "isel: non-zero const ptr not supported");
                        self.emit_bank_select(pa);
                        self.emit(format!("    CLRF 0x{:02X}", self.file_reg(pa)));
                        self.emit_bank_select(pa + 1);
                        self.emit(format!("    CLRF 0x{:02X}", self.file_reg(pa + 1)));
                    }
                    Val::Reg(r) if !self.resolved.contains_key(&ssa_key(self.cur_func, r)) => {
                        // A runtime pointer value: copy the lo byte; hi is 0.
                        let sa = self.slot_addr(self.cur_func, r).direct();
                        self.emit_w_load(sa);
                        self.emit_w_store(pa);
                        self.emit("    MOVLW 0x00".to_string());
                        self.emit_w_store(pa + 1);
                    }
                    Val::Reg(r) if matches!(self.resolved_for(r), (Base::Global(_), _, ref t) if t.is_empty()) =>
                    {
                        let (base, k, _) = self.resolved_for(r);
                        let Base::Global(name) = &base else {
                            unreachable!()
                        };
                        let addr = self.global_addr(name) + u16::from(k);
                        assert!(
                            addr <= 0x3F,
                            "isel: ptr arg address 0x{addr:02X} exceeds the 6-bit data space"
                        );
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                        self.emit_w_store(pa);
                        self.emit("    MOVLW 0x00".to_string());
                        self.emit_w_store(pa + 1);
                    }
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        let sa = match &base {
                            Base::Global(name) => {
                                let base_addr = self.global_addr(name);
                                let k_lo = (u16::from(k) & 0xFF) as u8;
                                match terms.as_slice() {
                                    [] => {
                                        let addr = base_addr + u16::from(k);
                                        assert!(
                                            addr <= 0x3F,
                                            "isel: ptr arg address 0x{addr:02X} exceeds the 6-bit data space"
                                        );
                                        self.emit(format!(
                                            "    MOVLW 0x{:02X}",
                                            (addr & 0x3F) as u8
                                        ));
                                        self.emit_w_store(pa);
                                        self.emit("    MOVLW 0x00".to_string());
                                        self.emit_w_store(pa + 1);
                                        continue;
                                    }
                                    [(1, reg)] => {
                                        let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                        self.emit(format!(
                                            "    MOVLW 0x{:02X}",
                                            (base_addr & 0x3F) as u8
                                        ));
                                        self.emit_bank_select(ra);
                                        self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra)));
                                        if k_lo != 0 {
                                            self.emit_add_w_lit(k_lo);
                                        }
                                        self.emit_w_store(pa);
                                        self.emit("    MOVLW 0x00".to_string());
                                        self.emit_w_store(pa + 1);
                                        continue;
                                    }
                                    _ => panic!("isel: plain ptr arg with multiple terms not yet supported: {terms:?}"),
                                }
                            }
                            Base::Slot(sname, false) if self.param_holds_addr(sname) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                            Base::Slot(sname, false) => {
                                // A plain alloca slot: the slot IS the
                                // object, so its address is a compile-time
                                // constant (6-bit).
                                let addr = self.slot_addr(self.cur_func, sname).direct();
                                assert!(
                                    addr <= 0x3F,
                                    "isel: alloca slot 0x{addr:02X} exceeds the 6-bit data space"
                                );
                                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0x3F) as u8));
                                self.emit_w_store(pa);
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit_w_store(pa + 1);
                                continue;
                            }
                            Base::Slot(sname, true) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                        };
                        let k_lo = (u16::from(k) & 0xFF) as u8;
                        match terms.as_slice() {
                            [] => {
                                self.emit_w_load(sa);
                                if k_lo != 0 {
                                    self.emit_add_w_lit(k_lo);
                                }
                                self.emit_w_store(pa);
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit_w_store(pa + 1);
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                self.emit_w_load(sa);
                                self.emit_bank_select(ra);
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(ra)));
                                if k_lo != 0 {
                                    self.emit_add_w_lit(k_lo);
                                }
                                self.emit_w_store(pa);
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit_w_store(pa + 1);
                            }
                            _ => panic!("isel: plain ptr arg with multiple terms not yet supported: {terms:?}"),
                        }
                    }
                }
            } else {
                let aty = arg.ty.expect("isel: scalar call arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, aty, pa);
                // A narrow scalar into a wide (4-byte) param only happens
                // for the float conversion routines, which land in P7.
                if aty.bytes() < callee.params[i].width {
                    panic!("isel: narrow scalar arg {i} of @{func} into a wide param lands in P7 (docs/37)");
                }
            }
        }
    }

    /// `dst = call func(args)`: copy each arg into the callee's slots,
    /// `CALL func`, then copy the retval slots (0x08+) into `dst`. No
    /// PCLATH on the baseline; every CALL target must sit in the low 256
    /// words of its page (enforced by the 0x100-word program assert).
    fn emit_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        if !callees.is_empty() {
            self.emit_indirect_call(dst, ty, func, args, callees);
            return;
        }
        if !self.is_function(func) {
            let l_trap = self.fresh_label();
            self.emit(format!("{l_trap}:"));
            self.emit(format!("    GOTO {l_trap}"));
            return;
        }
        self.emit_call_args(func, args);
        self.emit(format!("    CALL {func}"));
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                self.emit_bank_select(self.retval_lo + u16::from(i));
                self.emit(format!(
                    "    MOVF 0x{:02X}, W",
                    self.retval_lo + u16::from(i)
                ));
                self.emit_w_store(da + u16::from(i));
            }
        }
    }

    /// `dst = call %fp(args)` through a function pointer: an inline
    /// compare-and-call chain over the candidate set. The compare pairs
    /// (XORLW then BTFSS) never have a memory operand between the flag-set
    /// and the skip target, so no reassert can land between them.
    fn emit_indirect_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        let fp = self.slot_addr(self.cur_func, func).direct();
        let l_done = self.fresh_label();
        for cand in callees.iter() {
            let l_next = self.fresh_label();
            self.emit_bank_select(fp);
            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(fp)));
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit_bank_select(fp + 1);
            self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(fp + 1)));
            self.emit(format!("    XORLW HIGH({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit_call_args(cand, args);
            self.emit(format!("    CALL {cand}"));
            self.emit(format!("    GOTO {l_done}"));
            self.emit(format!("{l_next}:"));
        }
        let l_trap = self.fresh_label();
        self.emit(format!("{l_trap}:"));
        self.emit(format!("    GOTO {l_trap}"));
        self.emit(format!("{l_done}:"));
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                self.emit_bank_select(self.retval_lo + u16::from(i));
                self.emit(format!(
                    "    MOVF 0x{:02X}, W",
                    self.retval_lo + u16::from(i)
                ));
                self.emit_w_store(da + u16::from(i));
            }
        }
    }
    fn emit_inst(&mut self, i: &Inst) {
        self.cur_loc = i.loc().cloned();
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                if let Some(g) = l.ptr.strip_prefix('@') {
                    // A global read stays untracked (volatile sources stay
                    // sound): raw MOVF with an explicit reassert, then the
                    // trackable store into the SSA value's own slot.
                    let src = self.global_addr(g);
                    for k in 0..l.ty.bytes() {
                        self.emit_bank_select(src + u16::from(k));
                        self.emit(format!(
                            "    MOVF 0x{:02X}, W",
                            self.file_reg(src + u16::from(k))
                        ));
                        self.emit_w_store(dst + u16::from(k));
                    }
                } else if l.ptr.starts_with("0x") {
                    // A literal (SFR) pointer from `inttoptr`: a direct MOVF.
                    // dst is the SSA value's own slot, so the store side is
                    // trackable even though the read is not.
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit_bank_select(base + u16::from(k));
                        self.emit(format!(
                            "    MOVF 0x{:02X}, W",
                            self.file_reg(base + u16::from(k))
                        ));
                        self.emit_w_store(dst + u16::from(k));
                    }
                } else {
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!(
                            "isel: pointer {:?} is not @global, %reg or a literal",
                            l.ptr
                        )
                    });
                    let ptr = Val::Reg(r.to_string());
                    for k in 0..l.ty.bytes() {
                        self.emit_ptr_load_byte(&ptr, k);
                        self.emit_w_store(dst + u16::from(k));
                    }
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel: only i8/i16 stores supported");
                if let Some(g) = s.ptr.strip_prefix('@') {
                    let dst = self.global_addr(g);
                    self.emit_move_val_to_slot(&s.val, s.ty, dst);
                } else if s.ptr.starts_with("0x") {
                    // A literal (SFR) pointer from `inttoptr`.
                    let base = literal_ptr_addr(&s.ptr);
                    for k in 0..s.ty.bytes() {
                        self.emit_load_byte(&s.val, k);
                        self.emit_bank_select(base + u16::from(k));
                        self.emit(format!(
                            "    MOVWF 0x{:02X}",
                            self.file_reg(base + u16::from(k))
                        ));
                    }
                } else {
                    let r = s.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!(
                            "isel: pointer {:?} is not @global, %reg or a literal",
                            s.ptr
                        )
                    });
                    let (base, _, _) = self.resolved_for(r);
                    if let Base::Global(name) = &base {
                        assert!(
                            !self.global_is_const(name),
                            "isel: store to const (flash) global @{name}"
                        );
                    }
                    let ptr = Val::Reg(r.to_string());
                    for k in 0..s.ty.bytes() {
                        self.emit_ptr_store_byte(&ptr, k, &s.val);
                    }
                }
            }
            Inst::Gep(_) => {}    // virtual: lowered at each load/store use
            Inst::Alloca(_) => {} // virtual: the slot is sized by alloc
            Inst::Memcpy(m) => match &m.len {
                MemLen::Const(n) => {
                    // Byte loop over the same pointer machinery: each byte
                    // re-resolves both pointers (and reasserts FSR) exactly
                    // like a per-byte load/store.
                    for i in 0..*n {
                        self.emit_ptr_load_byte(&m.src, i);
                        self.emit_ptr_store_w(&m.dst, i);
                    }
                }
                MemLen::Reg(_) => panic!("isel: dynamic memcpy lands in P4 (docs/37)"),
            },
            Inst::Bin(b) => {
                assert!(
                    b.ty != Ty::I1 || matches!(b.op, BinOp::And | BinOp::Or | BinOp::Xor),
                    "isel: only i8/i16 binops supported (and i1 And/Or/Xor)"
                );
                let b_ty = if b.ty == Ty::I1 { Ty::I8 } else { b.ty };
                let da = self.slot_addr(self.cur_func, &b.dst).direct();
                match (b.op, b_ty) {
                    (BinOp::Add, Ty::I16) => self.emit_add16(&b.a, &b.b, da),
                    (BinOp::Add, Ty::I32) => panic!("isel: i32 arithmetic lands in P6 (docs/37)"),
                    (BinOp::Add, Ty::I8) => {
                        // Normalize: a const LHS swaps to the RHS so the
                        // const-adder arm is used, never reading a const as
                        // a file-register address.
                        let (a, b_op) = match (&b.a, &b.b) {
                            (Val::Const(_), Val::Const(_)) => {
                                panic!("isel: constant folding not implemented")
                            }
                            (Val::Const(_), _) => (&b.b, &b.a),
                            _ => (&b.a, &b.b),
                        };
                        match b_op {
                            Val::Const(k) => {
                                let kb = (*k & 0xFF) as u8;
                                let aa = self.val_addr(a).direct();
                                self.emit_bank_select(aa);
                                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(aa)));
                                self.emit(format!("    MOVWF 0x{:02X}", self.tmp));
                                self.emit(format!("    MOVLW 0x{kb:02X}"));
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.tmp));
                                self.emit_w_store(da);
                            }
                            _ => {
                                let (aa, bb) =
                                    (self.val_addr(a).direct(), self.val_addr(b_op).direct());
                                self.emit_bank_select(bb);
                                self.emit(format!("    MOVF 0x{:02X}, W", self.file_reg(bb)));
                                self.emit_bank_select(aa);
                                self.emit(format!("    ADDWF 0x{:02X}, W", self.file_reg(aa)));
                                self.emit_w_store(da);
                            }
                        }
                    }
                    (BinOp::And, Ty::I8) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW")
                    }
                    (BinOp::And, Ty::I16) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW")
                    }
                    (BinOp::And, Ty::I32) => panic!("isel: i32 arithmetic lands in P6 (docs/37)"),
                    (BinOp::Or, Ty::I8) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW")
                    }
                    (BinOp::Or, Ty::I16) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW")
                    }
                    (BinOp::Or, Ty::I32) => panic!("isel: i32 arithmetic lands in P6 (docs/37)"),
                    (BinOp::Xor, Ty::I8) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW")
                    }
                    (BinOp::Xor, Ty::I16) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW")
                    }
                    (BinOp::Xor, Ty::I32) => panic!("isel: i32 arithmetic lands in P6 (docs/37)"),
                    // sub is NOT commutative: a const LHS (d = k - a) takes
                    // the staged idiom (no SUBLW).
                    (BinOp::Sub, Ty::I8) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 1);
                        } else {
                            self.emit_sub8(&b.a, &b.b, da);
                        }
                    }
                    (BinOp::Sub, Ty::I16) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 2);
                        } else {
                            self.emit_sub16(&b.a, &b.b, da);
                        }
                    }
                    (BinOp::Sub, Ty::I32) => panic!("isel: i32 arithmetic lands in P6 (docs/37)"),
                    // mul/div/rem reach isel only via hand-written IR:
                    // legalize rewrites every one into a runtime routine
                    // call. Panics: a legalize miss must never silently
                    // miscompile.
                    (BinOp::Mul, _) => {
                        panic!("isel: mul lands in P6 (docs/37); legalize must rewrite it to a routine call")
                    }
                    (BinOp::UDiv, _) => {
                        panic!("isel: udiv lands in P6 (docs/37); legalize must rewrite it to a routine call")
                    }
                    (BinOp::URem, _) => {
                        panic!("isel: urem lands in P6 (docs/37); legalize must rewrite it to a routine call")
                    }
                    (BinOp::SDiv, _) => {
                        panic!("isel: sdiv lands in P6 (docs/37); legalize must rewrite it to a routine call")
                    }
                    (BinOp::SRem, _) => {
                        panic!("isel: srem lands in P6 (docs/37); legalize must rewrite it to a routine call")
                    }
                    // Shifts with a const count inline as a fixed RLF/RRF
                    // sequence; k >= width is LLVM poison and panics. A
                    // variable (reg) count must never reach isel: legalize
                    // rewrites it to the routine call.
                    (BinOp::Shl, _) | (BinOp::LShr, _) | (BinOp::AShr, _) => {
                        if b.ty == Ty::I32 {
                            panic!("isel: i32 shifts land in P6 (docs/37)");
                        }
                        let width = b.ty.bytes() as i64 * 8;
                        let k = match &b.b {
                            Val::Const(k) => *k,
                            other => panic!(
                                "isel: variable-count {:?} shift lands in P6 (docs/37) (count {other:?}); legalize must rewrite it to a routine call",
                                b.op
                            ),
                        };
                        assert!(
                            (0..width).contains(&k),
                            "isel: const shift count {k} out of range [0, {width}) (LLVM poison)"
                        );
                        self.emit_move_val_to_slot(&b.a, b.ty, da);
                        let n = b.ty.bytes();
                        for _ in 0..k {
                            match b.op {
                                BinOp::Shl => {
                                    self.emit("    BCF STATUS, 0".to_string());
                                    for i in 0..n {
                                        self.emit_bank_select(da + u16::from(i));
                                        self.emit(format!(
                                            "    RLF 0x{:02X}, F",
                                            self.file_reg(da + u16::from(i))
                                        ));
                                    }
                                }
                                BinOp::LShr => {
                                    self.emit("    BCF STATUS, 0".to_string());
                                    for i in (0..n).rev() {
                                        self.emit_bank_select(da + u16::from(i));
                                        self.emit(format!(
                                            "    RRF 0x{:02X}, F",
                                            self.file_reg(da + u16::from(i))
                                        ));
                                    }
                                }
                                BinOp::AShr => {
                                    // Set C from the sign bit before each
                                    // RRF so the sign fills every vacated
                                    // bit. The reassert precedes the first
                                    // test (BCF/BSF touch only STATUS).
                                    let hi = da + u16::from(n - 1);
                                    self.emit_bank_select(hi);
                                    self.emit(format!("    BTFSC 0x{:02X}, 7", self.file_reg(hi)));
                                    self.emit("    BSF STATUS, 0".to_string());
                                    self.emit(format!("    BTFSS 0x{:02X}, 7", self.file_reg(hi)));
                                    self.emit("    BCF STATUS, 0".to_string());
                                    for i in (0..n).rev() {
                                        self.emit_bank_select(da + u16::from(i));
                                        self.emit(format!(
                                            "    RRF 0x{:02X}, F",
                                            self.file_reg(da + u16::from(i))
                                        ));
                                    }
                                }
                                _ => unreachable!(),
                            }
                        }
                    }
                    _ => panic!("isel: unsupported binop for milestone 2"),
                }
            }
            // freeze is a no-op in the backend: copy `val` byte-for-byte into
            // the dst slot.
            Inst::Freeze(f) => {
                let da = self.slot_addr(self.cur_func, &f.dst).direct();
                self.emit_move_val_to_slot(&f.val, f.ty, da);
            }
            Inst::Zext(z) => {
                assert!(z.from.bytes() <= z.to.bytes(), "isel: zext must not narrow");
                let da = self.slot_addr(self.cur_func, &z.dst).direct();
                for i in 0..z.from.bytes() {
                    self.emit_load_byte(&z.val, i);
                    self.emit_w_store(da + u16::from(i));
                }
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit_bank_select(da + u16::from(i));
                    self.emit(format!(
                        "    CLRF 0x{:02X}",
                        self.file_reg(da + u16::from(i))
                    ));
                }
            }
            Inst::IntToPtr(p) => {
                // A runtime integer address becoming a pointer VALUE: copy
                // the address bytes into the dst slot, which iselcore seeded
                // as an indirect pointer.
                assert_eq!(
                    p.from, p.to,
                    "isel: inttoptr must keep the byte width (i16 -> ptr)"
                );
                let da = self.slot_addr(self.cur_func, &p.dst).direct();
                for i in 0..p.from.bytes() {
                    self.emit_load_byte(&p.val, i);
                    self.emit_w_store(da + u16::from(i));
                }
            }
            Inst::Sext(x) => {
                assert!(
                    x.from != Ty::I1 && x.from.bytes() < x.to.bytes(),
                    "isel: sext only supports i8/i16 -> i16/i32 (i1 sign-fill is undefined)"
                );
                assert!(
                    !matches!(&x.val, Val::Const(_)),
                    "isel: sext of a constant not supported (constant folding not implemented)"
                );
                let da = self.slot_addr(self.cur_func, &x.dst).direct();
                for i in 0..x.from.bytes() {
                    self.emit_load_byte(&x.val, i);
                    self.emit_w_store(da + u16::from(i));
                }
                let src_hi = x.from.bytes() - 1;
                let a = self.val_addr(&x.val).direct();
                let l_pos = self.fresh_label();
                let l_fill = self.fresh_label();
                self.emit_bank_select(a + u16::from(src_hi));
                self.emit(format!(
                    "    BTFSS 0x{:02X}, 7",
                    self.file_reg(a + u16::from(src_hi))
                ));
                self.emit(format!("    GOTO {l_pos}"));
                self.emit("    MOVLW 0xFF".to_string());
                self.emit(format!("    GOTO {l_fill}"));
                self.emit(format!("{l_pos}:"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit(format!("{l_fill}:"));
                for i in x.from.bytes()..x.to.bytes() {
                    self.emit_w_store(da + u16::from(i));
                }
            }
            Inst::Trunc(t) => {
                assert!(
                    t.from.bytes() > t.to.bytes() || (t.to == Ty::I1 && t.from != Ty::I1),
                    "isel: trunc must narrow"
                );
                let da = self.slot_addr(self.cur_func, &t.dst).direct();
                for i in 0..t.to.bytes() {
                    self.emit_load_byte(&t.val, i);
                    self.emit_w_store(da + u16::from(i));
                }
                if t.to == Ty::I1 {
                    // Every i1 consumer tests the whole byte for nonzero, so
                    // the truncated-away bits have to go.
                    self.emit("    MOVLW 0x01".to_string());
                    self.emit_bank_select(da);
                    self.emit(format!("    ANDWF 0x{:02X}, F", self.file_reg(da)));
                }
            }
            Inst::Icmp(ic) => {
                let da = self.slot_addr(self.cur_func, &ic.dst).direct();
                match ic.pred.as_str() {
                    "eq" => {
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("Z", da);
                    }
                    "ne" => {
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("!Z", da);
                    }
                    pred => {
                        let (signed, need_z) = match pred {
                            "ult" | "uge" => (false, false),
                            "ugt" | "ule" => (false, true),
                            "slt" | "sge" => (true, false),
                            "sgt" | "sle" => (true, true),
                            _ => panic!("isel: unknown icmp predicate {pred:?}"),
                        };
                        self.emit_cmp_c(&ic.a, &ic.b, ic.ty, signed);
                        // A multi-byte borrow chain ends with a byte-level
                        // Z; full equality needs the XOR accumulation, which
                        // preserves C.
                        if need_z && ic.ty.bytes() > 1 {
                            self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        }
                        let mat = match pred {
                            "ult" | "slt" => "!C",
                            "uge" | "sge" => "C",
                            "ugt" | "sgt" => "C&&!Z",
                            _ => "!C||Z",
                        };
                        self.emit_materialize(mat, da);
                    }
                }
            }
            Inst::Select(s) => {
                if s.ptr {
                    if matches!((&s.a, &s.b), (Val::Const(_), Val::Const(_))) {
                        self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                    } else if self.select_is_seeded(&s.dst) {
                        self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                    } else {
                        // A folded pointer select is virtual (lowered at each
                        // load/store use); it emits nothing.
                    }
                } else {
                    self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                }
            }
            Inst::Call(c) => self.emit_call(&c.dst, c.ty, &c.func, &c.args, &c.callees),
            Inst::VaStart(_) => panic!("isel: variadic functions land in P4 (docs/37)"),
            Inst::VaArg(_) => panic!("isel: variadic functions land in P4 (docs/37)"),
            Inst::Asm(a) => {
                self.emit("; --- asm start ---".to_string());
                let substituted = self.substitute_asm(&a.template, &a.operands);
                for line in substituted.split('\n') {
                    self.emit(line.to_string());
                }
                self.emit("; --- asm end ---".to_string());
            }
            Inst::FloatBin(_) | Inst::Fcmp(_) | Inst::FloatConv(_) => {
                panic!("isel: float instructions land in P7 (docs/37)")
            }
            _ => panic!("isel: unsupported instruction for milestone 2"),
        }
    }
    fn emit_terminator(&mut self, t: &Inst, labels: &HashMap<String, String>) {
        self.cur_loc = t.loc().cloned();
        match t {
            Inst::Br(br) => {
                let l = &labels[&br.target];
                self.emit(format!("    GOTO {l}"));
            }
            Inst::BrCond(b) => {
                let lt = &labels[&b.t];
                let lf = &labels[&b.f];
                self.emit_cond_branch(&b.cond, lt, lf);
            }
            // No RETURN on the baseline: every return leaves through a
            // `RETLW 0`, void or valued (RETLW always clobbers W with its
            // literal, so a valued return parks the value in the fixed
            // retval slots first).
            Inst::Ret(None, _) => self.emit("    RETLW 0x00".to_string()),
            Inst::Ret(Some((ty, v)), _) => {
                for i in 0..ty.bytes() {
                    self.emit_load_byte(v, i);
                    self.emit_w_store(self.retval_lo + u16::from(i));
                }
                self.emit("    RETLW 0x00".to_string());
            }
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }
}

/// Block-dominator sets, for the phi-copy back-edge classifier.
fn block_dominators(f: &ir::Func) -> HashMap<String, HashSet<String>> {
    let entry = &f.blocks[0].label;
    let all: HashSet<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
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

/// Emit one function's body. Runtime routines panic (P6); naked functions
/// go verbatim; ordinary functions get block labels, phi copies, and
/// terminators. The baseline core has no interrupts: an `isr` function
/// never reaches here (asserted in `select_with_locs`).
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    g.cur_loc = None;
    if ir::is_runtime_routine(&f.name) {
        panic!("isel: runtime routine @{} lands in P6 (docs/37)", f.name);
    }
    if f.naked {
        g.emit(format!("{}:", f.name));
        g.emit("; --- asm start ---".to_string());
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Asm(a) => {
                        let substituted = g.substitute_asm(&a.template, &a.operands);
                        for line in substituted.split('\n') {
                            g.emit(line.to_string());
                        }
                    }
                    _ => panic!(
                        "isel: naked function '{}' contains non-asm instruction; naked bodies must be pure assembly",
                        f.name
                    ),
                }
            }
        }
        g.emit("; --- asm end ---".to_string());
        g.emit("".to_string());
        return;
    }
    // Block label scheme: the entry block uses the bare function name (so
    // CALLs and GOTOs resolve to it); every other block is
    // `{func}_L{label}`.
    let mut labels: HashMap<String, String> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let lbl = if i == 0 {
            f.name.clone()
        } else {
            format!("{}_L{}", f.name, b.label)
        };
        labels.insert(b.label.clone(), lbl);
    }
    // phi elimination, keyed by the (predecessor, merge) edge.
    let mut phi_copies: HashMap<(String, String), Vec<(String, Ty, Val)>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Phi(p) = i {
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
    for b in f.blocks.iter() {
        g.emit(format!("{}:", labels[&b.label]));
        let mut terminator = None;
        for i in &b.insts {
            match i {
                Inst::Phi(_) => {} // eliminated; copies emitted at pred ends
                Inst::Br(_) | Inst::BrCond(_) | Inst::Ret(..) => terminator = Some(i),
                _ => g.emit_inst(i),
            }
        }
        if let Some(t) = terminator {
            match t {
                Inst::Br(br) => {
                    let merge = br.target.clone();
                    if let Some(c) = phi_copies.get(&(b.label.clone(), merge.clone())) {
                        g.emit(format!("    ; phi copies for pred {0}", labels[&b.label]));
                        emit_phi_copies(g, c, doms[&b.label].contains(&merge));
                    }
                    g.emit(format!("    GOTO {}", labels[&merge]));
                }
                Inst::BrCond(bc) => {
                    let lt = labels[&bc.t].clone();
                    let lf = labels[&bc.f].clone();
                    let t_copies = phi_copies.get(&(b.label.clone(), bc.t.clone()));
                    let f_copies = phi_copies.get(&(b.label.clone(), bc.f.clone()));
                    match &bc.cond {
                        Val::Reg(r) => {
                            let ca = g.val_addr(&Val::Reg(r.clone())).direct();
                            g.emit_bank_select(ca);
                            g.emit(format!("    MOVF 0x{:02X}, W", g.file_reg(ca)));
                            match (t_copies, f_copies) {
                                (None, None) => {
                                    g.emit("    BTFSC STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                                (Some(ct), Some(cf)) => {
                                    let lcop = g.fresh_label();
                                    g.emit("    BTFSS STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lcop}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, cf, doms[&b.label].contains(&bc.f));
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!("{lcop}:"));
                                    emit_phi_copies(g, ct, doms[&b.label].contains(&bc.t));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                                (_, Some(c)) => {
                                    g.emit("    BTFSS STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lt}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.f));
                                    g.emit(format!("    GOTO {lf}"));
                                }
                                (Some(c), None) => {
                                    g.emit("    BTFSC STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.t));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                            }
                        }
                        Val::Const(k) => {
                            if *k != 0 {
                                if let Some(c) = t_copies {
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.t));
                                }
                                g.emit(format!("    GOTO {lt}"));
                            } else {
                                if let Some(c) = f_copies {
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.f));
                                }
                                g.emit(format!("    GOTO {lf}"));
                            }
                        }
                        Val::Global(_) => panic!("isel: conditional branch on a global"),
                    }
                }
                _ => g.emit_terminator(t, &labels),
            }
        }
    }
    g.emit("".to_string());
}

/// Emit the dependency-ordered phi copies for one (pred -> merge) edge: a
/// copy never overwrites a slot a later copy still needs to read. The
/// ordering is the edge's CFG position (dominance): reader-first on a back
/// edge, writer-first on a forward edge. A true cycle panics.
fn emit_phi_copies<'m>(g: &mut Gen<'m>, copies: &[(String, Ty, Val)], back_edge: bool) {
    let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
        .iter()
        .map(|(dst, ty, val)| {
            let da = g.slot_addr(g.cur_func, dst).direct();
            let src = match val {
                Val::Reg(r) => {
                    if g.resolved.contains_key(&ssa_key(g.cur_func, r)) {
                        None
                    } else {
                        Some(g.slot_addr(g.cur_func, r).direct())
                    }
                }
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
            panic!("isel: cyclic phi copies not supported");
        }
    }
}

/// The word size of emitted lines: 1 word per instruction line (labels,
/// `.align`/`.table` directives, `equ` lines, comments, and blanks are 0),
/// mirroring the asm crate's pass-1 counting so the CALL-ceiling assert
/// matches the addresses the assembler will assign.
fn word_size(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|raw| {
            let line = raw.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                return false;
            }
            if line.starts_with("list") || line.starts_with("radix") {
                return false;
            }
            if line.starts_with("org ") {
                return false;
            }
            if line.starts_with("end") {
                return false;
            }
            if line.ends_with(':') {
                return false;
            }
            if line.contains(" equ ") {
                return false;
            }
            if line.starts_with(".align ") {
                return false;
            }
            if line.starts_with(".table ") {
                return false;
            }
            true
        })
        .count()
}

/// Select instructions for the whole module, producing baseline assembly
/// text. Single pass: no page assignment (no PCLATH), no banking pass (the
/// D-2 reasserts are emitted inline by isel), bodies in module order.
///
/// `addrs` is the complete address map from `alloc`: globals by name,
/// locals by `{func}::{name}`. The icmp scratch byte, the four retval
/// bytes, and the two staging bytes live in fixed common RAM: the
/// allocation places no locals in common RAM, so no collision.
pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    select_with_locs(device, m, addrs).0
}

/// `select` plus a parallel per-line source-location vector, index-aligned
/// with the returned asm text. `None` marks a compiler-generated line. The
/// driver threads this to the address-to-line table.
pub fn select_with_locs(
    device: &Device,
    m: &Module,
    addrs: &HashMap<String, u16>,
) -> (String, Vec<Option<SrcLoc>>) {
    let mut out: Vec<String> = Vec::new();
    let mut locs: Vec<Option<SrcLoc>> = Vec::new();
    // The baseline core has no interrupts: an `isr` function never reaches
    // the backend (the interrupt phase was dropped from the baseline port
    // in docs/37).
    for f in &m.funcs {
        assert!(
            !f.isr,
            "isel: the baseline core has no interrupts (docs/37)"
        );
    }
    // The fixed staging layout in common RAM: scratch 0x07, retval
    // 0x08-0x0B, tmp 0x0C, tmp2 0x0D. All bank-independent, so no reassert
    // is ever needed for them.
    let (common_lo, common_hi) = device
        .common_ram
        .expect("isel-pic-baseline's fixed scratch/retval/tmp layout needs a common-RAM region");
    let scratch: u16 = common_lo;
    let retval_lo: u16 = common_lo + 1;
    let tmp: u16 = common_lo + 5;
    let tmp2: u16 = common_lo + 6;
    assert!(
        retval_lo + 4 <= common_hi + 1,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must fit in common RAM",
        retval_lo + 3
    );
    assert!(
        tmp2 + 1 <= common_hi + 1,
        "isel: tmp staging 0x{tmp:02X}-0x{tmp2:02X} must fit in common RAM"
    );
    out.extend(vec![
        "; pic8 -- integer spine milestone 2 (isel-pic-baseline)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR    equ 0x04".to_string(),
        "INDF   equ 0x00".to_string(),
        "PCL    equ 0x02".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ]);
    locs.extend(std::iter::repeat(None).take(11));
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
    // Const globals copied to RAM (alloc moved them to `addrs`) need their
    // bytes initialized before main runs. Each MOVWF carries its own D-2
    // reassert: the FSR bank on entry to `__start` is whatever reset left.
    let mut init: Vec<String> = Vec::new();
    for g in &m.globals {
        if g.is_const && addrs.contains_key(&g.name) {
            let base = addrs[&g.name];
            for (i, b) in g.bytes.iter().enumerate() {
                if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                    let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                    init.push(format!("    MOVLW {lit}({f})"));
                } else {
                    init.push(format!("    MOVLW 0x{b:02X}"));
                }
                init.extend(bank_select_lines(device, base + i as u16));
                init.push(format!(
                    "    MOVWF 0x{:02X}",
                    file_reg(device, base + i as u16)
                ));
            }
        }
    }
    let mut start_block: Vec<String> = vec!["__start:".to_string()];
    start_block.extend(init);
    start_block.extend([
        "    CALL main".to_string(),
        "    SLEEP".to_string(),
        "".to_string(),
    ]);
    let start_len = start_block.len();
    out.extend(start_block);
    locs.extend(std::iter::repeat(None).take(start_len));
    // Pointers resolve eagerly: every GEP chain folds to `(base, k, terms)`
    // keyed `{func}::{reg}`. The fold is shared with the other backends in
    // `iselcore::resolve_pointers`.
    let resolved = resolve_pointers(m);
    // Fresh-label counter at module scope: labels are file-scoped in the
    // single `.asm` output, so it must not reset per function.
    let mut label_counter = 0u32;
    let mut body_words = 0usize;
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            device,
            resolved: &resolved,
            scratch,
            retval_lo,
            tmp,
            tmp2,
            cur_func: &f.name,
            label_counter: &mut label_counter,
            w_holds: None,
            cur_loc: None,
            out: Vec::new(),
            locs: Vec::new(),
        };
        emit_func_body(&mut g, f);
        body_words += word_size(&g.out);
        out.extend(g.out);
        locs.extend(g.locs);
    }
    // The baseline CALL ceiling: CALL forces PC bit 8 to 0 (DS41236E 4.7),
    // so every CALL target must sit in the low 256 words of its page. P2
    // places all code in page 0's low half: the reset GOTO (1 word), the
    // `__start` init (2 words per RAM-copied const byte) plus its
    // CALL/SLEEP (2 words), and the bodies must total 0x100 words or less.
    // A program past the ceiling panics (multi-page placement is D-5/P4).
    let init_words: usize = m
        .globals
        .iter()
        .filter(|g| g.is_const && addrs.contains_key(&g.name))
        .map(|g| 2 * g.bytes.len())
        .sum();
    let total = 1 + init_words + 2 + body_words;
    assert!(
        total <= 0x100,
        "isel: program of {total} words exceeds the 256-word CALL ceiling (0x100); multi-page placement lands in P4 (docs/37)"
    );
    out.push("    end".to_string());
    locs.push(None);
    (out.join("\n"), locs)
}

/// `parse_map` lives in `iselcore`: a plain text-format parser over
/// `alloc`'s output with nothing core-specific about it. Re-exported here
/// so the binary (`src/bin/isel-pic-baseline.rs`) keeps working.
pub use iselcore::parse_map;
