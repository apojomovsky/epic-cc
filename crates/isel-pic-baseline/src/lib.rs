//! `isel-pic-baseline`: instruction selection for the PIC baseline
//! (12-bit-word) integer spine.
//!
//! Baseline's ISA is a strict subset of classic PIC14's (docs/37 §1): the
//! same byte/bit/literal three-way field vocabulary, missing exactly
//! `SUBLW` and `RETURN`. So this crate forks classic `isel`'s integer-spine
//! structure (the `Gen` struct, `emit_inst`/`emit_terminator`/
//! `emit_func_body`, the `MOVF`/`MOVWF`/`ADDLW`/`ADDWF`/`BTFSC`/`GOTO`
//! idiom vocabulary) but owns its instruction-emission code, exactly as
//! `isel-pic14e` does (D-1, resolved in P2).
//!
//! The three ISA deltas from classic `isel`:
//! 1. No `SUBLW` (D-6): `w := k - f` lowers to `MOVWF tmp` / `MOVLW k` /
//!    `SUBWF tmp,W` needing one scratch register.
//! 2. No `RETURN` (D-6): every return is `RETLW 0` (void) or `RETLW k`
//!    (valued, after copying the value to the retval slots).
//! 3. Destination default d = 1 (file), matching gpasm on this core
//!    (P1 confirmed); classic isel defaults to W.
//!
//! The load-bearing new code is D-2's unconditional `FSR` bank-bit
//! reassertion: on baseline, `FSR<5>` is the bank select for BOTH direct
//! and indirect addressing, and `FSR` is also the only indirect pointer.
//! `emit_bank_select` emits `BCF`/`BSF FSR,5` before every direct access
//! to a non-zero bank and before every `INDF` touch, unconditionally, the
//! same way classic PIC14 re-emits IRP inside `isel` rather than tracking
//! state in `crates/banking` (docs/37 §2 D-2). There is no banking pass
//! for this core.
//!
//! No PCLATH: baseline's PC<9> comes from STATUS PA0 (bit 5), and
//! CALL/RETLW are hard-limited to the low 256 words of a page (D-5). P2's
//! fixtures fit in page 0, so PA0 stays 0 and no page management is
//! emitted. No interrupts on this core: no ISR emission, no RETFIE.
//!
//! Every address comes from the caller map: globals by name, locals by
//! `{func}::{name}`. isel allocates no slots. A missing value panics:
//! the map owns layout.

use device::Device;
use ir::{BinOp, Inst, MemLen, Module, SrcLoc, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};
use std::collections::{HashMap, HashSet};

/// The byte address of a literal-pointer operand (`"0x<K>"`, the
/// `inttoptr (<ty> <k> to ptr)` constant-pointer form parsed by irparse).
/// Used for direct (SFR) load/store: the register is bank-mirrored
/// (0x00-0x06 on baseline), so no FSR setup and no bank reassertion.
fn literal_ptr_addr(ptr: &str) -> u16 {
    let lit = ptr
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("isel: malformed literal pointer {ptr:?}"));
    u16::from_str_radix(lit, 16)
        .unwrap_or_else(|_| panic!("isel: malformed literal pointer {ptr:?}"))
}

/// How a single-byte pointer access completes after `emit_ptr_setup`.
enum Addr {
    /// A plain file register (the address is statically known).
    Direct(u16),
    /// FSR is set up; the access goes through INDF.
    Indirect,
}

/// Per-function codegen state. All addresses come from the module-wide map;
/// `cur_func` selects the current function's local entries.
struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    device: &'m Device,
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)`: GEP chains fully collapsed, plus the
    /// seeded pointer bases (byval/sret params and allocas). `gep`/`alloca`
    /// themselves emit nothing; each `load`/`store`/`memcpy` through a
    /// pointer reg lowers the pointer at its use.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    scratch: u16,
    /// A second fixed common-RAM temp, dedicated to the ADDLW-replacement
    /// idioms (baseline has no literal-add op, D-6): `W = W + k` and the
    /// carry/borrow folds stash W here. Never used elsewhere, so it is
    /// always free at a fold (the primary `scratch` byte is live across
    /// cmp accumulation and GEP offsets).
    scratch2: u16,
    retval_lo: u16,
    cur_func: &'m str,
    /// Module-scoped fresh-label counter, shared across every function so the
    /// emitted `tmp{n}:` labels stay unique in the single `.asm` output.
    tmp: &'m mut u32,
    /// Tracks the slot address whose value also sits in W, or `None`.
    /// Collapses a store followed by a reload of the same slot (epic-cc#214).
    /// Plain `emit` clears the cache, so staleness cannot cross other
    /// emission.
    w_holds: Option<u16>,
    /// The source location of the instruction currently being emitted, or
    /// `None` for compiler-generated glue (prologue, `__start`).
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

    /// `MOVWF addr`, unless `addr` is already known to hold W's value from
    /// an immediately preceding `emit_w_store`/`emit_w_load` of the same
    /// address (a genuine no-op then: the byte there already equals W).
    /// Marks `addr` as holding W's value either way.
    fn emit_w_store(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            self.emit_bank_select(addr);
            self.emit(format!("    MOVWF 0x{addr:02X}"));
        }
        self.w_holds = Some(addr);
    }

    /// `MOVF addr, W`, unless `addr`'s value is already known to be in W.
    /// Marks `addr` as holding W's value either way.
    fn emit_w_load(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            self.emit_bank_select(addr);
            self.emit(format!("    MOVF 0x{addr:02X}, W"));
        }
        self.w_holds = Some(addr);
    }

    /// `W = W + k` without `ADDLW` (baseline has no literal-add op, D-6):
    /// stash W in the scratch2 byte, load k, `ADDWF scratch2, W` computes
    /// scratch2 + k. scratch2 is dedicated to these folds, so it is always
    /// free here.
    fn emit_add_w_const(&mut self, k: u8) {
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch2));
        self.emit(format!("    MOVLW 0x{k:02X}"));
        self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch2));
    }

    /// `W = W + 1` when the carry flag is set (the i16 add carry fold),
    /// without `ADDLW`. `INCF scratch2, F` sets Z but not C; the caller's
    /// next op (ADDWF/SUBWF) sets C/Z fresh.
    fn emit_add_w_carry(&mut self) {
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch2));
        self.emit("    BTFSC STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", self.scratch2));
        self.emit(format!("    MOVF 0x{:02X}, W", self.scratch2));
    }

    /// `W = W + 1` when the carry flag is clear (the i16 sub borrow fold).
    fn emit_add_w_borrow(&mut self) {
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch2));
        self.emit("    BTFSS STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", self.scratch2));
        self.emit(format!("    MOVF 0x{:02X}, W", self.scratch2));
    }

    /// `W = W + 1` when the carry flag is set, then `W = W + k` (the i16
    /// add carry fold plus a constant high byte), without `ADDLW`.
    fn emit_add_w_carry_const(&mut self, k: u8) {
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch2));
        self.emit("    BTFSC STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", self.scratch2));
        self.emit(format!("    MOVLW 0x{k:02X}"));
        self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch2));
    }

    /// D-2's unconditional `FSR` bank-bit reassertion (docs/37 §2 D-2):
    /// on baseline, `FSR<5>` is the bank select for both direct and
    /// indirect addressing. A direct operand whose physical address is a
    /// banked GPR needs `FSR<5>` set to its bank before the access; the
    /// shared GPR (0x07-0x0F) and the SFR block (0x00-0x06) are
    /// bank-independent and need no reassertion. Emitted unconditionally,
    /// no dataflow tracking, exactly like classic PIC14's IRP reassertion.
    fn emit_bank_select(&mut self, addr: u16) {
        match self.device.bank_of(addr) {
            Some(0) => self.emit("    BCF FSR, 5".to_string()),
            Some(1) => self.emit("    BSF FSR, 5".to_string()),
            _ => {}
        }
    }

    /// Resolve `{func}::{name}` to its base byte address (lo for multi-byte).
    fn slot_addr(&self, func: &str, name: &str) -> Slot {
        Slot::Direct(
            *self
                .addrs
                .get(&ssa_key(func, name))
                .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}")),
        )
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
            Val::Const(k) => {
                // Mask to the byte: clang prints i8 constants >= 128 as
                // negative i8 (found by the fuzz corpus); the value is the
                // same mod 256.
                Slot::Direct((*k & 0xFF) as u16)
            }
        }
    }

    /// The byte address of a RAM global (from the map).
    fn global_addr(&self, name: &str) -> u16 {
        *self
            .addrs
            .get(name)
            .unwrap_or_else(|| panic!("isel: no address for @{name}"))
    }

    /// Whether `name` is a const (flash) global: read via RETLW tables.
    /// A const that was copied to RAM (alloc placed it in `addrs`) is
    /// treated as RAM.
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
    /// than a RAM/const global.
    fn is_function(&self, name: &str) -> bool {
        self.m.funcs.iter().any(|f| f.name == name)
    }

    /// True when `name` is a plain pointer param of the current function,
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
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

    /// A fresh local label for intra-block jumps (select branches).
    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.tmp);
        *self.tmp += 1;
        s
    }

    /// W = byte `idx` of `val`.
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
                    // A pointer VALUE (a GEP over a global, or a runtime
                    // address slot): materialize its address bytes. Baseline
                    // pointers are 6-bit flat FSR values, so only byte 0
                    // carries the address; byte 1 is always 0.
                    if let Base::Global(name) = &base {
                        let addr = self.global_addr(name).wrapping_add(k as u16);
                        let lo = (addr & 0xFF) as u8;
                        match terms.as_slice() {
                            [] => {
                                self.emit(format!(
                                    "    MOVLW 0x{:02X}",
                                    if idx == 0 { lo } else { 0 }
                                ));
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                if idx == 0 {
                                    self.emit(format!("    MOVLW 0x{lo:02X}"));
                                    self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                                } else {
                                    self.emit("    MOVLW 0x00".to_string());
                                }
                            }
                            _ => {
                                assert!(
                                    terms.len() == 2 && terms.iter().all(|(sc, _)| *sc == 1),
                                    "isel: multi-term GEP load with {terms:?} not supported"
                                );
                                let ra1 = self.val_addr(&Val::Reg(terms[0].1.clone())).direct();
                                let ra2 = self.val_addr(&Val::Reg(terms[1].1.clone())).direct();
                                if idx == 0 {
                                    self.emit(format!("    MOVLW 0x{lo:02X}"));
                                    self.emit(format!("    ADDWF 0x{ra1:02X}, W"));
                                    self.emit(format!("    ADDWF 0x{ra2:02X}, W"));
                                } else {
                                    self.emit("    MOVLW 0x00".to_string());
                                }
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
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                if k != 0 {
                                    self.emit_add_w_const(k);
                                }
                            } else {
                                self.emit("    MOVLW 0x00".to_string());
                            }
                        }
                        [(1, reg)] => {
                            let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                            } else {
                                self.emit("    MOVLW 0x00".to_string());
                            }
                        }
                        _ => {
                            assert!(
                                terms.len() == 2 && terms.iter().all(|(sc, _)| *sc == 1),
                                "isel: multi-term GEP load with {terms:?} not supported"
                            );
                            let ra1 = self.val_addr(&Val::Reg(terms[0].1.clone())).direct();
                            let ra2 = self.val_addr(&Val::Reg(terms[1].1.clone())).direct();
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra1:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra2:02X}, W"));
                            } else {
                                self.emit("    MOVLW 0x00".to_string());
                            }
                        }
                    }
                    return;
                }

                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_w_load(a + u16::from(idx));
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    // A function's address is a link-time label literal:
                    // byte 0 = LOW(g), byte 1 = HIGH(g) (epic-cc#73).
                    let lit = if idx == 0 { "LOW" } else { "HIGH" };
                    self.emit(format!("    MOVLW {lit}({g})"));
                } else {
                    // A data global in value position is a pointer ADDRESS:
                    // materialize it as literals, never read the pointee's
                    // contents (epic-cc#155).
                    let a = self.val_addr(&Val::Global(g.clone())).direct();
                    let b = ((a >> (idx as u32 * 8)) & 0xFF) as u8;
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
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone())).direct();
                self.emit_bank_select(a + u16::from(idx));
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
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

    /// Set the Z flag to (a == b) without disturbing other flags.
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
                    self.emit(format!("    XORWF 0x{addr:02X}, W"));
                } else {
                    self.emit_bank_select(addr);
                    self.emit(format!("    MOVF 0x{addr:02X}, W"));
                }
            }
        }
    }

    /// Set C = (a >= b), unsigned or signed (sign-bit complement). For i8
    /// the SUBWF also leaves Z = (a == b); wider borrow chains leave only a
    /// byte-level Z, so equality appends `emit_cmp_eq` (C intact).
    /// Baseline has no `SUBLW`, so a const LHS lowers to the scratch idiom
    /// `MOVWF tmp` / `MOVLW k` / `SUBWF tmp,W` (D-6).
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
                // No SUBLW: `k - W` via the scratch idiom. W holds the b
                // byte; stash it, load k, SUBWF computes k - W, so
                // C = (a >= b).
                self.emit_load_cmp_byte(b, 0, signed, high);
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                let k0 = (k & 0xFF) as u8;
                let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
                self.emit(format!("    MOVLW 0x{k0:02X}"));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            }
            _ => {
                if n > 1 {
                    self.emit_cmp_c_file_lhs_wide(a, b, n, high, signed);
                    return;
                }
                let aa = self.val_addr(a).direct();
                let use_scratch = signed; // signed file-LHS: SUBWF's file operand must be a ^ 0x80
                if use_scratch {
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit_bank_select(aa + high as u16);
                    self.emit(format!("    XORWF 0x{:02X}, W", aa + high as u16));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                }
                self.emit_load_cmp_byte(b, 0, signed, high);
                self.emit_bank_select(if use_scratch && n == 1 {
                    self.scratch
                } else {
                    aa
                });
                self.emit(format!(
                    "    SUBWF 0x{:02X}, W",
                    if use_scratch && n == 1 {
                        self.scratch
                    } else {
                        aa
                    }
                ));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit_add_w_borrow();
                    let f = if i == high && use_scratch {
                        self.scratch
                    } else {
                        aa + i as u16
                    };
                    self.emit_bank_select(f);
                    self.emit(format!("    SUBWF 0x{f:02X}, W"));
                }
            }
        }
    }

    /// The multi-byte (n > 1, i16) borrow chain for `C = (a >= b)` with a
    /// file-LHS `a`. Same wrap-correct INCFSZ folds as classic isel; the
    /// const-LHS path uses the scratch idiom instead of SUBLW.
    fn emit_cmp_c_file_lhs_wide(&mut self, a: &Val, b: &Val, n: u8, high: u8, signed: bool) {
        let aa = self.val_addr(a).direct();
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit_bank_select(aa);
        self.emit(format!("    SUBWF 0x{aa:02X}, W"));
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
                        self.emit(format!("    XORWF 0x{addr:02X}, W"));
                    }
                }
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(aa + u16::from(high));
                self.emit(format!("    XORWF 0x{:02X}, W", aa + u16::from(high)));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                self.emit_bank_select(self.scratch);
                self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            } else {
                match b {
                    Val::Const(k) => {
                        let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    }
                    _ => {
                        self.emit_load_cmp_byte(b, i, signed, high);
                        let addr = self.val_addr(b).direct() + u16::from(i);
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit_bank_select(addr);
                        self.emit(format!("    INCFSZ 0x{addr:02X}, W"));
                    }
                }
                self.emit_bank_select(aa + u16::from(i));
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + u16::from(i)));
            }
        }
    }

    /// The multi-byte (n > 1, i16) const-LHS borrow chain. Baseline has no
    /// `SUBLW`, so each byte's `k_i - (b_i + borrow)` lowers to the scratch
    /// idiom: stash the b byte, load k_i, SUBWF computes k_i - W.
    fn emit_cmp_c_const_lhs_wide(&mut self, k: &i64, b: &Val, n: u8, high: u8, signed: bool) {
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        let k0 = (k & 0xFF) as u8;
        let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
        self.emit(format!("    MOVLW 0x{k0:02X}"));
        self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
        for i in 1..n {
            if signed && i == high {
                let addr = self.val_addr(b).direct() + u16::from(high);
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(addr);
                self.emit(format!("    XORWF 0x{addr:02X}, W"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.retval_lo));
            } else {
                let addr = self.val_addr(b).direct() + u16::from(i);
                self.emit_bank_select(addr);
                self.emit(format!("    MOVF 0x{addr:02X}, W"));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{addr:02X}, W"));
                let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            }
        }
    }

    /// Materialize a flag predicate into `dst` as an i1.
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
        self.emit_w_store(dst);
    }

    /// Branch on `cond`: Z = (cond == 0); if Z is set (cond == 0) go to `f`,
    /// otherwise (cond != 0) go to `t`.
    fn emit_cond_branch(&mut self, cond: &Val, t: &str, f: &str) {
        match cond {
            Val::Reg(r) => {
                let ca = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(ca);
                self.emit(format!("    MOVF 0x{ca:02X}, W"));
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
    /// indirect slot (`Base::Slot(_, true)`).
    fn select_is_seeded(&self, name: &str) -> bool {
        matches!(
            self.resolved.get(&ssa_key(self.cur_func, name)),
            Some((Base::Slot(_, true), 0, t)) if t.is_empty()
        )
    }

    /// Copy the two-byte ADDRESS VALUE of `val` into the slot at `dst`.
    fn emit_move_addr_to_slot(&mut self, val: &Val, dst: u16) {
        match val {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit_w_store(dst);
                self.emit(format!("    MOVLW 0x{:02X}", ((k >> 8) & 0xFF) as u8));
                self.emit_w_store(dst + 1);
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    self.emit(format!("    MOVLW LOW({g})"));
                    self.emit_w_store(dst);
                    self.emit(format!("    MOVLW HIGH({g})"));
                    self.emit_w_store(dst + 1);
                } else {
                    let addr = self.global_addr(g);
                    self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                    self.emit_w_store(dst);
                    self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
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
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit_w_store(dst);
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        self.emit_w_store(dst + 1);
                        return;
                    }
                    other => panic!("isel: cannot materialize {other:?} as a select arm"),
                };
                self.emit_bank_select(sa);
                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                self.emit_w_store(dst);
                self.emit_bank_select(sa + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                self.emit_w_store(dst + 1);
            }
        }
    }

    /// `d = cond ? a : b` via an if/else jump over two copies.
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
        self.emit(format!("    MOVF 0x{ca:02X}, W"));
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

    /// `d = a + b` for i16.
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
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit_bank_select(ra);
                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                self.emit_w_store(dst);
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit_add_w_carry();
                self.emit_bank_select(ra + 1);
                self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                self.emit_w_store(dst + 1);
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit_bank_select(ra);
                self.emit(format!("    MOVF 0x{ra:02X}, W"));
                self.emit_add_w_const(lo);
                self.emit_w_store(dst);
                self.emit_bank_select(ra + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", ra + 1));
                self.emit_add_w_carry_const(hi);
                self.emit_w_store(dst + 1);
            }
            Val::Global(_) => panic!("isel: add16 with a global operand"),
        }
    }

    /// `d = a OP b` bytewise, for the commutative binops and/or/xor at i8 or
    /// i16.
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

    /// `d = a - b` for i8.
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (*k & 0xFF) as u8));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit_w_store(dst);
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit_w_store(dst);
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = k - a` (const LHS) for `bytes`-wide values. Baseline has no
    /// `SUBLW`, so each byte's `k_i - (a_i + borrow)` lowers to the scratch
    /// idiom: stash the a byte, load k_i, SUBWF computes k_i - W (D-6).
    fn emit_sub_const_lhs(&mut self, k: &i64, a: &Val, dst: u16, bytes: u8) {
        let aa = self.val_addr(a).direct();
        self.emit_bank_select(aa);
        self.emit(format!("    MOVF 0x{aa:02X}, W"));
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
        self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
        self.emit_w_store(dst);
        for i in 1..bytes {
            let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
            self.emit_bank_select(aa + u16::from(i));
            self.emit(format!("    MOVF 0x{:02X}, W", aa + u16::from(i)));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            self.emit(format!("    MOVLW 0x{kb:02X}"));
            self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            self.emit_w_store(dst + u16::from(i));
        }
    }

    /// `d = a - b` for i16.
    fn emit_sub16(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{lo:02X}"));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit_w_store(dst);
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit_add_w_borrow();
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit_w_store(dst + 1);
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit_w_store(dst);
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit_add_w_borrow();
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit_w_store(dst + 1);
            }
            Val::Global(_) => panic!("isel: sub16 with a global operand"),
        }
    }

    /// How a byte access at `ptr + byte_off` completes: `Direct(a)` reads or
    /// writes the plain file register `a`; `Indirect` means FSR is already
    /// set up and the access goes through INDF. Emits the address setup for
    /// dynamic/indirect pointers. Const (flash) bases are rejected (loads
    /// take the RETLW path before this; stores panic).
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => {
                assert!(
                    !self.global_is_const(g),
                    "isel: store to const (flash) global @{g}"
                );
                Addr::Direct(self.global_addr(g) + u16::from(byte_off))
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
                            Addr::Direct(
                                self.global_addr(name) + u16::from(k) + u16::from(byte_off),
                            )
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
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
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

    /// `W = RAM[ptr + byte_off]`: one byte of a pointer load or a memcpy
    /// source. Direct bases read the plain file register; dynamic bases set
    /// FSR first and read INDF.
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), k, terms) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        // Const (flash) reads are P4 (RETLW tables). P2 has
                        // no const reads; a const base panics loudly.
                        panic!(
                            "isel: const (flash) read of @{name} is P4 (RETLW tables); not supported in P2"
                        );
                    }
                    let _ = (k, terms);
                }
            }
            Val::Global(g) => {
                if self.global_is_const(g) {
                    panic!(
                        "isel: const (flash) read of @{g} is P4 (RETLW tables); not supported in P2"
                    );
                }
            }
            Val::Const(_) => panic!("isel: load through a constant pointer"),
        }
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{a:02X}, W"))
            }
            Addr::Indirect => {
                self.emit("    BCF FSR, 5".to_string());
                self.emit("    MOVF INDF, W".to_string())
            }
        }
    }

    /// `RAM[ptr + byte_off] = W`: the store side of a byte access.
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF 0x{a:02X}"))
            }
            Addr::Indirect => {
                self.emit("    BCF FSR, 5".to_string());
                self.emit("    MOVWF INDF".to_string())
            }
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_load_byte(val, byte_off);
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF 0x{a:02X}"));
            }
            Addr::Indirect => {
                self.emit_load_byte(val, byte_off);
                self.emit("    BCF FSR, 5".to_string());
                self.emit("    MOVWF INDF".to_string());
            }
        }
    }

    /// `FSR = base + k + byte_off + Σ terms`: baseline's FSR is the full
    /// 6-bit flat address (bank bits and offset together, D-2 item 2), so
    /// one MOVLW/MOVWF loads it. No FSR0H/FSR0L split, no linear alias, no
    /// IRP.
    fn emit_fsr_to(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let lit = (u16::from(base_addr) + u16::from(k) + u16::from(byte_off)) & 0x3F;
        match terms {
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit_add_w_const(lit as u8);
                self.emit("    MOVWF FSR".to_string());
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit_add_w_const(lit as u8);
                self.emit("    MOVWF FSR".to_string());
            }
        }
    }

    /// Indirect (sret) FSR setup: `FSR = [slot] + k + byte_off + Σ terms`.
    /// The slot holds the target address (the caller stores LOW then HIGH
    /// of it into the two slot bytes). Baseline's FSR is 6 bits, so only the
    /// low byte of the stored address is used; the high byte is ignored.
    fn emit_fsr_indirect(&mut self, slot_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0x3F,
            "isel: indirect offset k {k} + off {byte_off} out of 6-bit range"
        );
        if terms.is_empty() {
            self.emit_bank_select(slot_addr);
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit_add_w_const(kk as u8);
            self.emit("    MOVWF FSR".to_string());
        } else {
            self.emit_accum_terms(terms);
            self.emit_bank_select(slot_addr);
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
            self.emit_add_w_const(kk as u8);
            self.emit("    MOVWF FSR".to_string());
        }
    }

    /// `scratch = Σ scale×%reg`.
    fn emit_accum_terms(&mut self, terms: &[(u8, String)]) {
        self.emit("    MOVLW 0x00".to_string());
        self.emit_w_store(self.scratch);
        for (scale, r) in terms {
            let a = self.val_addr(&Val::Reg(r.clone())).direct();
            for _ in 0..*scale {
                self.emit_bank_select(a);
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit_w_store(self.scratch);
            }
        }
    }

    /// `dst = call %fp(args)` through a function pointer: an inline
    /// compare-and-call chain over the candidate set.
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
            self.emit(format!("    MOVF 0x{fp:02X}, W"));
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit_bank_select(fp + 1);
            self.emit(format!("    MOVF 0x{:02X}, W", fp + 1));
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

    /// `dst = call func(args)`.
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

    /// Copy the call arguments into the callee's param slots.
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
                panic!("isel: variadic call to @{func} not supported in P2");
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
                    self.emit_bank_select(pa + u16::from(b));
                    self.emit(format!("    MOVWF 0x{:02X}", pa + u16::from(b)));
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
                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                self.emit_bank_select(pa);
                self.emit(format!("    MOVWF 0x{:02X}", pa));
                self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                self.emit_bank_select(pa + 1);
                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
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
                            self.emit_bank_select(pa);
                            self.emit(format!("    MOVWF 0x{:02X}", pa));
                            self.emit(format!("    MOVLW HIGH({g})"));
                            self.emit_bank_select(pa + 1);
                            self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                        } else {
                            if self.global_is_const(g) {
                                panic!("isel: const global @{g} too large for RAM copy");
                            }
                            let addr = self.global_addr(g);
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                            self.emit_bank_select(pa);
                            self.emit(format!("    MOVWF 0x{:02X}", pa));
                            self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                            self.emit_bank_select(pa + 1);
                            self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                        }
                    }
                    Val::Const(c) => {
                        assert_eq!(*c, 0, "isel: non-zero const ptr not supported");
                        self.emit_bank_select(pa);
                        self.emit(format!("    CLRF 0x{:02X}", pa));
                        self.emit_bank_select(pa + 1);
                        self.emit(format!("    CLRF 0x{:02X}", pa + 1));
                    }
                    Val::Reg(r) => {
                        // A runtime pointer value: copy its two address bytes.
                        let sa = self.slot_addr(self.cur_func, r).direct();
                        self.emit_bank_select(sa);
                        self.emit(format!("    MOVF 0x{sa:02X}, W"));
                        self.emit_bank_select(pa);
                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                        self.emit_bank_select(sa + 1);
                        self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                        self.emit_bank_select(pa + 1);
                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                    }
                }
            } else {
                let aty = arg.ty.expect("isel: scalar arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, aty, pa);
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
                    let src = self.global_addr(g);
                    for k in 0..l.ty.bytes() {
                        self.emit_bank_select(src + u16::from(k));
                        self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(k)));
                        self.emit_w_store(dst + u16::from(k));
                    }
                } else if l.ptr.starts_with("0x") {
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit_bank_select(base + u16::from(k));
                        self.emit(format!("    MOVF 0x{:02X}, W", base + u16::from(k)));
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
                    let base = literal_ptr_addr(&s.ptr);
                    for k in 0..s.ty.bytes() {
                        self.emit_load_byte(&s.val, k);
                        self.emit_bank_select(base + u16::from(k));
                        self.emit(format!("    MOVWF 0x{:02X}", base + u16::from(k)));
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
            Inst::Alloca(_) => {} // virtual: the slot is sized by alloc; lowered at each use
            Inst::Memcpy(m) => match &m.len {
                MemLen::Const(n) => {
                    for i in 0..*n {
                        self.emit_ptr_load_byte(&m.src, i);
                        self.emit_ptr_store_w(&m.dst, i);
                    }
                }
                MemLen::Reg(_) => panic!("isel: dynamic memcpy not supported in P2"),
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
                    (BinOp::Add, Ty::I8) => {
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
                                self.emit(format!("    MOVF 0x{aa:02X}, W"));
                                self.emit_add_w_const(kb);
                                self.emit_w_store(da);
                            }
                            _ => {
                                let (aa, bb) =
                                    (self.val_addr(a).direct(), self.val_addr(b_op).direct());
                                self.emit_bank_select(bb);
                                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                                self.emit_bank_select(aa);
                                self.emit(format!("    ADDWF 0x{aa:02X}, W"));
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
                    (BinOp::Or, Ty::I8) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW")
                    }
                    (BinOp::Or, Ty::I16) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW")
                    }
                    (BinOp::Xor, Ty::I8) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW")
                    }
                    (BinOp::Xor, Ty::I16) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW")
                    }
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
                    (BinOp::Add, Ty::I32) | (BinOp::Sub, Ty::I32) => {
                        panic!("isel: i32 arithmetic is P6 (long); not supported in P2")
                    }
                    (BinOp::Mul, _) => {
                        panic!("isel: mul reached isel; legalize must rewrite it to a routine call")
                    }
                    (BinOp::UDiv, _) => panic!(
                        "isel: udiv reached isel; legalize must rewrite it to a routine call"
                    ),
                    (BinOp::URem, _) => panic!(
                        "isel: urem reached isel; legalize must rewrite it to a routine call"
                    ),
                    (BinOp::SDiv, _) => panic!(
                        "isel: sdiv reached isel; legalize must rewrite it to a routine call"
                    ),
                    (BinOp::SRem, _) => panic!(
                        "isel: srem reached isel; legalize must rewrite it to a routine call"
                    ),
                    (BinOp::Shl, _) | (BinOp::LShr, _) | (BinOp::AShr, _) => {
                        panic!("isel: shift reached isel; legalize must rewrite it to a routine call (P6)")
                    }
                    _ => panic!("isel: unsupported binop for P2"),
                }
            }
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
                    self.emit(format!("    CLRF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::IntToPtr(p) => {
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
                self.emit(format!("    BTFSS 0x{:02X}, 7", a + u16::from(src_hi)));
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
                    self.emit("    MOVLW 0x01".to_string());
                    self.emit_bank_select(da);
                    self.emit(format!("    ANDWF 0x{da:02X}, F"));
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
                        if need_z && ic.ty.bytes() > 1 {
                            self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        }
                        let mat = match pred {
                            "ult" | "slt" => "!C",
                            "uge" | "sge" => "C",
                            "ugt" | "sgt" => "C&&!Z",
                            _ => "!C||Z", // ule | sle
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
                        // A pointer-typed select is a pointer VALUE, folded by
                        // iselcore into the resolved map like a GEP: it emits
                        // nothing.
                    }
                } else {
                    self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                }
            }
            Inst::Call(c) => self.emit_call(&c.dst, c.ty, &c.func, &c.args, &c.callees),
            Inst::VaStart(_) | Inst::VaArg(_) => {
                panic!("isel: variadic functions not supported in P2")
            }
            Inst::Asm(a) => {
                self.emit("; --- asm start ---".to_string());
                for line in a.template.split('\n') {
                    self.emit(line.to_string());
                }
                self.emit("; --- asm end ---".to_string());
            }
            Inst::FloatBin(_) | Inst::Fcmp(_) | Inst::FloatConv(_) => {
                panic!("isel: float instructions are not code-generated yet (P7 soft-float)")
            }
            _ => panic!("isel: unsupported instruction for P2"),
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
            Inst::Ret(None, _) => self.emit("    RETLW 0x00".to_string()),
            Inst::Ret(Some((ty, v)), _) => {
                for i in 0..ty.bytes() {
                    self.emit_load_byte(v, i);
                    self.emit_bank_select(self.retval_lo + u16::from(i));
                    self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + u16::from(i)));
                }
                self.emit("    RETLW 0x00".to_string());
            }
            _ => panic!("isel: unsupported terminator for P2"),
        }
    }
}

/// Back-edge classifier for the phi-copy ordering.
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

/// Emit one function's body into `g.out`.
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    g.cur_loc = None;
    if ir::is_runtime_routine(&f.name) {
        panic!(
            "isel: runtime routine @{} reached isel; mul/div/shift routines are P6",
            f.name
        );
    }
    if f.naked {
        g.emit(format!("{}:", f.name));
        g.emit("; --- asm start ---".to_string());
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Asm(a) => {
                        for line in a.template.split('\n') {
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
    let mut labels: HashMap<String, String> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let lbl = if i == 0 {
            f.name.clone()
        } else {
            format!("{}_L{}", f.name, b.label)
        };
        labels.insert(b.label.clone(), lbl);
    }
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
                            g.emit(format!("    MOVF 0x{ca:02X}, W"));
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
                Inst::Ret(..) => g.emit_terminator(t, &labels),
                _ => unreachable!(),
            }
        }
    }
}

/// Emit the phi copies for one (predecessor, merge) edge.
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

/// P2's page model: the fixtures fit in page 0 (PA0 stays 0), so there is
/// no page management to verify. A no-op that keeps the driver's
/// `verify_page_fit` call site uniform across cores. P4 (const tables)
/// owns the real page model.
pub fn verify_page_fit(_m: &Module, _asm: &str) {}

/// Assemble the module into `.asm` text.
pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    select_with_locs(device, m, addrs).0
}

/// Extends `select` with per-line source locations for the driver address
/// table. Marks generated lines with `None`.
pub fn select_with_locs(
    device: &Device,
    m: &Module,
    addrs: &HashMap<String, u16>,
) -> (String, Vec<Option<SrcLoc>>) {
    let mut out: Vec<String> = Vec::new();
    let mut locs: Vec<Option<SrcLoc>> = Vec::new();
    // The icmp scratch byte and the four retval bytes are fixed common-RAM
    // constants (bank-independent, the device's common RAM is never used by
    // locals, so no collision). Baseline's common RAM is 0x07-0x0F (9
    // bytes): scratch at 0x07, retval at 0x08-0x0B, leaving 0x0C-0x0F free.
    let (common_lo, common_hi) = device
        .common_ram
        .expect("isel's fixed scratch/retval layout needs a common-RAM region");
    let scratch: u16 = common_lo;
    let retval_lo: u16 = common_lo + 1;
    // scratch2 (the ADDLW-replacement temp) sits right after the retval
    // region: common RAM 0x07-0x0F = scratch(0x07) + retval(0x08-0x0B) +
    // scratch2(0x0C), leaving 0x0D-0x0F free.
    let scratch2: u16 = common_lo + 5;
    assert!(
        retval_lo + 4 <= common_hi + 1,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must fit in common RAM",
        retval_lo + 3
    );
    assert!(
        scratch2 <= common_hi,
        "isel: scratch2 0x{scratch2:02X} must fit in common RAM (0x{common_lo:02X}-0x{common_hi:02X})"
    );
    out.extend(vec![
        "; pic8 -- PIC baseline integer spine (isel-pic-baseline)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "INDF   equ 0x00".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR    equ 0x04".to_string(),
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
    // Const string literals copied to RAM need their bytes initialized
    // before main runs.
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
                init.push(format!("    MOVWF 0x{:02X}", base + i as u16));
            }
        }
    }
    let start_block: Vec<String> = vec![
        "__start:".to_string(),
        "    CALL main".to_string(),
        "    SLEEP".to_string(),
        "".to_string(),
    ];
    let mut start_full: Vec<String> = Vec::new();
    start_full.extend(init);
    start_full.extend(start_block);
    let start_len = start_full.len();
    out.extend(start_full);
    locs.extend(std::iter::repeat(None).take(start_len));
    // Pointers resolve eagerly: every GEP chain folds to `(base, k, terms)`.
    let resolved = resolve_pointers(m);
    let mut tmp = 0u32;
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            device,
            resolved: &resolved,
            scratch,
            scratch2,
            retval_lo,
            cur_func: &f.name,
            tmp: &mut tmp,
            w_holds: None,
            cur_loc: None,
            out: Vec::new(),
            locs: Vec::new(),
        };
        emit_func_body(&mut g, f);
        out.extend(g.out);
        locs.extend(g.locs);
    }
    out.push("    end".to_string());
    locs.push(None);
    (out.join("\n"), locs)
}

/// `parse_map` lives in `iselcore` now: it is a plain text-format parser
/// over `alloc`'s output with nothing PIC14-specific about it. Re-exported
/// here so this crate's binary keeps working unchanged.
pub use iselcore::parse_map;
