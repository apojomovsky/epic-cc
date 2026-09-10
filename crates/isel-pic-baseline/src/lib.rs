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
//! CALL/RETLW are hard-limited to the low 256 words of a page (D-5).
//! Code lives in the page-0 low half; const tables pack the page-0 low
//! half first and spill to the page-1 low half with PA0 set/restore at
//! each const read. No interrupts on this core: no ISR emission, no RETFIE.
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

/// Whose page a PA0 placeholder bit must select, resolved after layout.
/// `Clone` so call sites can name both ends of a managed CALL.
#[derive(Clone)]
enum Pa0Page {
    /// A function, by name (`__start` maps to page 0).
    Func(String),
    /// A const table, by global name.
    Table(String),
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
    /// A third fixed common-RAM temp, dedicated to staging a store value
    /// across the pointer's FSR setup: the FSR setup clobbers W, and a
    /// banked value's load reasserts FSR<5>, which would clobber the
    /// just-set pointer FSR before the INDF store (silent wrong-bank
    /// write). Never used elsewhere, so it is always free at a store.
    store_tmp: u16,
    retval_lo: u16,
    cur_func: &'m str,
    /// PA0 fixups recorded during emission, patched after layout: each is
    /// the index (into this buffer's `out`) of a `BCF STATUS, 5`
    /// placeholder plus whose page its bit must select. One buffer's
    /// fixups patch that buffer only.
    fixups: &'m mut Vec<(usize, Pa0Page)>,
    /// Module-scoped fresh-label counter, shared across every function so the
    /// emitted `tmp{n}:` labels stay unique in the single `.asm` output.
    tmp: &'m mut u32,
    /// Tracks the slot address whose value also sits in W, or `None`.
    /// Collapses a store followed by a reload of the same slot (epic-cc#214).
    /// Plain `emit` clears the cache, so staleness cannot cross other
    /// emission.
    w_holds: Option<u16>,
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
            self.emit(format!("    MOVWF {}", self.fop(addr)));
        }
        self.w_holds = Some(addr);
    }

    /// `MOVF addr, W`, unless `addr`'s value is already known to be in W.
    /// Marks `addr` as holding W's value either way.
    fn emit_w_load(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            self.emit_bank_select(addr);
            self.emit(format!("    MOVF {}, W", self.fop(addr)));
        }
        self.w_holds = Some(addr);
    }

    /// `W = W + k` without `ADDLW` (baseline has no literal-add op, D-6):
    /// stash W in the scratch2 byte, load k, `ADDWF scratch2, W` computes
    /// scratch2 + k. scratch2 is dedicated to these folds, so it is always
    /// free here.
    fn emit_add_w_const(&mut self, k: u8) {
        self.emit(format!("    MOVWF {}", self.fop(self.scratch2)));
        self.emit(format!("    MOVLW 0x{k:02X}"));
        self.emit(format!("    ADDWF {}, W", self.fop(self.scratch2)));
    }

    /// `W = W + 1` when the carry flag is set (the i16 add carry fold),
    /// without `ADDLW`. `INCF scratch2, F` sets Z but not C; the caller's
    /// next op (ADDWF/SUBWF) sets C/Z fresh.
    fn emit_add_w_carry(&mut self) {
        self.emit(format!("    MOVWF {}", self.fop(self.scratch2)));
        self.emit("    BTFSC STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF {}, F", self.fop(self.scratch2)));
        self.emit(format!("    MOVF {}, W", self.fop(self.scratch2)));
    }

    /// `W = W + 1` when the carry flag is clear (the i16 sub borrow fold).
    fn emit_add_w_borrow(&mut self) {
        self.emit(format!("    MOVWF {}", self.fop(self.scratch2)));
        self.emit("    BTFSS STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF {}, F", self.fop(self.scratch2)));
        self.emit(format!("    MOVF {}, W", self.fop(self.scratch2)));
    }

    /// `W = W + 1` when the carry flag is set, then `W = W + k` (the i16
    /// add carry fold plus a constant high byte), without `ADDLW`.
    fn emit_add_w_carry_const(&mut self, k: u8) {
        self.emit(format!("    MOVWF {}", self.fop(self.scratch2)));
        self.emit("    BTFSC STATUS, 0 ; C".to_string());
        self.emit(format!("    INCF {}, F", self.fop(self.scratch2)));
        self.emit(format!("    MOVLW 0x{k:02X}"));
        self.emit(format!("    ADDWF {}, W", self.fop(self.scratch2)));
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

    /// The 5-bit direct-operand field for a physical address: baseline's
    /// direct addressing uses the within-bank offset (0x00-0x1F) with
    /// `FSR<5>` selecting the bank (D-2), so a bank-1 GPR at 0x30-0x3F is
    /// addressed as 0x00-0x0F with `BSF FSR,5`. The bank-independent SFR
    /// block (0x00-0x06) and shared GPR (0x07-0x0F) are below 0x10, so the
    /// mask is identity there.
    fn fop(&self, addr: u16) -> String {
        format!("0x{:02X}", addr & 0x1F)
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
                                    self.emit(format!("    ADDWF {}, W", self.fop(ra)));
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
                                    self.emit(format!("    ADDWF {}, W", self.fop(ra1)));
                                    self.emit(format!("    ADDWF {}, W", self.fop(ra2)));
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
                                self.emit(format!("    MOVF {}, W", self.fop(sa)));
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
                                self.emit(format!("    MOVF {}, W", self.fop(sa)));
                                self.emit(format!("    ADDWF {}, W", self.fop(ra)));
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
                                self.emit(format!("    MOVF {}, W", self.fop(sa)));
                                self.emit(format!("    ADDWF {}, W", self.fop(ra1)));
                                self.emit(format!("    ADDWF {}, W", self.fop(ra2)));
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
                self.emit(format!("    XORWF {}, W", self.fop(a + u16::from(idx))));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone())).direct();
                self.emit_bank_select(a + u16::from(idx));
                self.emit(format!("    XORWF {}, W", self.fop(a + u16::from(idx))));
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
        self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
        for i in 1..n {
            self.emit_load_byte(a, i);
            self.emit_xor_byte(b, i);
            self.emit(format!("    IORWF {}, W", self.fop(self.scratch)));
            self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
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
                    self.emit(format!("    XORWF {}, W", self.fop(addr)));
                } else {
                    self.emit_bank_select(addr);
                    self.emit(format!("    MOVF {}, W", self.fop(addr)));
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
                self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                let k0 = (k & 0xFF) as u8;
                let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
                self.emit(format!("    MOVLW 0x{k0:02X}"));
                self.emit(format!("    SUBWF {}, W", self.fop(self.scratch)));
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
                    self.emit(format!("    XORWF {}, W", self.fop(aa + high as u16)));
                    self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                }
                self.emit_load_cmp_byte(b, 0, signed, high);
                let sub_f = if use_scratch && n == 1 {
                    self.scratch
                } else {
                    aa
                };
                self.emit_bank_select(sub_f);
                self.emit(format!("    SUBWF {}, W", self.fop(sub_f)));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit_add_w_borrow();
                    let f = if i == high && use_scratch {
                        self.scratch
                    } else {
                        aa + i as u16
                    };
                    self.emit_bank_select(f);
                    self.emit(format!("    SUBWF {}, W", self.fop(f)));
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
        self.emit(format!("    SUBWF {}, W", self.fop(aa)));
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
                        self.emit(format!("    XORWF {}, W", self.fop(addr)));
                    }
                }
                self.emit(format!("    MOVWF {}", self.fop(self.retval_lo)));
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(aa + u16::from(high));
                self.emit(format!("    XORWF {}, W", self.fop(aa + u16::from(high))));
                self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                self.emit(format!("    MOVF {}, W", self.fop(self.retval_lo)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(self.retval_lo)));
                self.emit_bank_select(self.scratch);
                self.emit(format!("    SUBWF {}, W", self.fop(self.scratch)));
            } else {
                match b {
                    Val::Const(k) => {
                        let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                        self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
                    }
                    _ => {
                        self.emit_load_cmp_byte(b, i, signed, high);
                        let addr = self.val_addr(b).direct() + u16::from(i);
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit_bank_select(addr);
                        self.emit(format!("    INCFSZ {}, W", self.fop(addr)));
                    }
                }
                self.emit_bank_select(aa + u16::from(i));
                self.emit(format!("    SUBWF {}, W", self.fop(aa + u16::from(i))));
            }
        }
    }

    /// The multi-byte (n > 1, i16) const-LHS borrow chain. Baseline has no
    /// `SUBLW`, so each byte's `k_i - (b_i + borrow)` lowers to the scratch
    /// idiom: stash the b byte, load k_i, SUBWF computes k_i - W.
    fn emit_cmp_c_const_lhs_wide(&mut self, k: &i64, b: &Val, n: u8, high: u8, signed: bool) {
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
        let k0 = (k & 0xFF) as u8;
        let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
        self.emit(format!("    MOVLW 0x{k0:02X}"));
        self.emit(format!("    SUBWF {}, W", self.fop(self.scratch)));
        for i in 1..n {
            if signed && i == high {
                let addr = self.val_addr(b).direct() + u16::from(high);
                self.emit("    MOVLW 0x80".to_string());
                self.emit_bank_select(addr);
                self.emit(format!("    XORWF {}, W", self.fop(addr)));
                self.emit(format!("    MOVWF {}", self.fop(self.retval_lo)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(self.retval_lo)));
                let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    SUBWF {}, W", self.fop(self.retval_lo)));
            } else {
                let addr = self.val_addr(b).direct() + u16::from(i);
                self.emit_bank_select(addr);
                self.emit(format!("    MOVF {}, W", self.fop(addr)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(addr)));
                let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{kb:02X}"));
                self.emit(format!("    SUBWF {}, W", self.fop(self.scratch)));
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
                self.emit(format!("    MOVF {}, W", self.fop(ca)));
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
                self.emit(format!("    MOVF {}, W", self.fop(sa)));
                self.emit_w_store(dst);
                self.emit_bank_select(sa + 1);
                self.emit(format!("    MOVF {}, W", self.fop(sa + 1)));
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
        self.emit(format!("    MOVF {}, W", self.fop(ca)));
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
                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                self.emit_bank_select(ra);
                self.emit(format!("    ADDWF {}, W", self.fop(ra)));
                self.emit_w_store(dst);
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF {}, W", self.fop(bb + 1)));
                self.emit_add_w_carry();
                self.emit_bank_select(ra + 1);
                self.emit(format!("    ADDWF {}, W", self.fop(ra + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit_bank_select(ra);
                self.emit(format!("    MOVF {}, W", self.fop(ra)));
                self.emit_add_w_const(lo);
                self.emit_w_store(dst);
                self.emit_bank_select(ra + 1);
                self.emit(format!("    MOVF {}, W", self.fop(ra + 1)));
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
                    let f = ra + u16::from(i);
                    self.emit_bank_select(f);
                    self.emit(format!("    {op} {}, W", self.fop(f)));
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
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = k - a` (const LHS) for `bytes`-wide values. Baseline has no
    /// `SUBLW`, so each byte's `k_i - (a_i + borrow)` lowers to the scratch
    /// idiom: stash the a byte, load k_i, SUBWF computes k_i - W (D-6).
    /// Byte 0 has no borrow-in; each higher byte folds the borrow from the
    /// low byte with the wrap-correct INCFSZ idiom: `k_i` preloads into the
    /// dst, `a_i` copies to scratch, and `SUBWF` computes `k_i - (a_i +
    /// borrow)` in place. At the wrap the skip leaves dst at `k_i` with C as
    /// the true borrow-out (epic-cc#1).
    fn emit_sub_const_lhs(&mut self, k: &i64, a: &Val, dst: u16, bytes: u8) {
        let aa = self.val_addr(a).direct();
        self.emit_bank_select(aa);
        self.emit(format!("    MOVF {}, W", self.fop(aa)));
        self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
        self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
        self.emit(format!("    SUBWF {}, W", self.fop(self.scratch)));
        self.emit_w_store(dst);
        for i in 1..bytes {
            let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
            // Subtrahend to scratch (dst preload may overlay a), k_i into
            // dst, then W reloaded from scratch before the fold: on the
            // no-borrow path the skip leaves k_i in W, so SUBWF would
            // compute k_i - k_i without the reload.
            self.emit_bank_select(aa + u16::from(i));
            self.emit(format!("    MOVF {}, W", self.fop(aa + u16::from(i))));
            self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
            self.emit(format!("    MOVLW 0x{kb:02X}"));
            self.emit_w_store(dst + u16::from(i));
            self.emit(format!("    MOVF {}, W", self.fop(self.scratch)));
            self.emit("    BTFSS STATUS, 0 ; C".to_string());
            self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
            self.emit(format!("    SUBWF {}, F", self.fop(dst + u16::from(i))));
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
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit_add_w_borrow();
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF {}, W", self.fop(aa + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
                self.emit_bank_select(bb + 1);
                self.emit(format!("    MOVF {}, W", self.fop(bb + 1)));
                self.emit_add_w_borrow();
                self.emit_bank_select(aa + 1);
                self.emit(format!("    SUBWF {}, W", self.fop(aa + 1)));
                self.emit_w_store(dst + 1);
            }
            Val::Global(_) => panic!("isel: sub16 with a global operand"),
        }
    }
    /// `d = a + b` for i32: byte 0 adds with the carry out exact
    /// (ADDWF), then each higher byte folds the carry into a scratch
    /// copy of the addend and adds to the destination in place. The
    /// fold uses INCFSZ's skip rather than an add-carry: when the fold
    /// wraps, the skip leaves the destination at `a_i`, the correct
    /// mod-256 result, with C = carry-in = 1, the true carry-out.
    /// Byte 0's const form uses the D-6 add idiom (no ADDLW here).
    fn emit_add32(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add32 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                self.emit_bank_select(ra);
                self.emit(format!("    ADDWF {}, W", self.fop(ra)));
                self.emit_w_store(dst);
                for i in 1..4u8 {
                    // b_i is copied to scratch first (the dst preload may
                    // overlay b), then W is reloaded from it after the
                    // preload's MOVF clobbers W.
                    self.emit_bank_select(bb + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(bb + u16::from(i))));
                    self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                    self.emit_bank_select(ra + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(ra + u16::from(i))));
                    self.emit_bank_select(dst + u16::from(i));
                    self.emit(format!("    MOVWF {}", self.fop(dst + u16::from(i))));
                    self.emit(format!("    MOVF {}, W", self.fop(self.scratch)));
                    self.emit("    BTFSC STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
                    self.emit(format!("    ADDWF {}, F", self.fop(dst + u16::from(i))));
                }
            }
            Val::Const(k) => {
                self.emit_bank_select(ra);
                self.emit(format!("    MOVF {}, W", self.fop(ra)));
                self.emit_add_w_const((k & 0xFF) as u8);
                self.emit_w_store(dst);
                for i in 1..4u8 {
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit_bank_select(ra + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(ra + u16::from(i))));
                    self.emit_bank_select(dst + u16::from(i));
                    self.emit(format!("    MOVWF {}", self.fop(dst + u16::from(i))));
                    self.emit(format!("    MOVLW 0x{kb:02X}"));
                    self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                    self.emit("    BTFSC STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
                    self.emit(format!("    ADDWF {}, F", self.fop(dst + u16::from(i))));
                }
            }
            Val::Global(_) => panic!("isel: add32 with a global operand"),
        }
    }

    /// `d = a - b` for i32: byte 0 subtracts with the borrow out exact
    /// (SUBWF), then each higher byte folds the borrow into a scratch
    /// copy of the subtrahend and subtracts from the destination in
    /// place. When the fold wraps the skip leaves the destination at
    /// `a_i` with C = borrow-in = 0, the true borrow-out.
    fn emit_sub32(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
                for i in 1..4u8 {
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit_bank_select(aa + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(aa + u16::from(i))));
                    self.emit_bank_select(dst + u16::from(i));
                    self.emit(format!("    MOVWF {}", self.fop(dst + u16::from(i))));
                    self.emit(format!("    MOVLW 0x{kb:02X}"));
                    self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
                    self.emit(format!("    SUBWF {}, F", self.fop(dst + u16::from(i))));
                }
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit_bank_select(bb);
                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                self.emit_bank_select(aa);
                self.emit(format!("    SUBWF {}, W", self.fop(aa)));
                self.emit_w_store(dst);
                for i in 1..4u8 {
                    // b_i is copied to scratch first (the dst preload may
                    // overlay b), then W is reloaded from it after the
                    // preload's MOVF clobbers W.
                    self.emit_bank_select(bb + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(bb + u16::from(i))));
                    self.emit(format!("    MOVWF {}", self.fop(self.scratch)));
                    self.emit_bank_select(aa + u16::from(i));
                    self.emit(format!("    MOVF {}, W", self.fop(aa + u16::from(i))));
                    self.emit_bank_select(dst + u16::from(i));
                    self.emit(format!("    MOVWF {}", self.fop(dst + u16::from(i))));
                    self.emit(format!("    MOVF {}, W", self.fop(self.scratch)));
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ {}, W", self.fop(self.scratch)));
                    self.emit(format!("    SUBWF {}, F", self.fop(dst + u16::from(i))));
                }
            }
            Val::Global(_) => panic!("isel: sub32 with a global operand"),
        }
    }

    /// Const-count `shl`/`lshr`/`ashr`: copy the value into dst, then
    /// rotate in place k times. shl goes lo to hi (carry up), lshr hi
    /// to lo (bits down), ashr sets C from the sign bit before each
    /// rrf so the sign fills every vacated bit. The bank select sits
    /// before each access but never between a skip test and its branch.
    fn emit_shift_const(&mut self, op: BinOp, a: &Val, ty: Ty, k: i64, dst: u16) {
        let width = ty.bytes() as i64 * 8;
        assert!(
            (0..width).contains(&k),
            "isel: const shift count {k} out of range [0, {width}) (LLVM poison)"
        );
        self.emit_move_val_to_slot(a, ty, dst);
        let n = ty.bytes();
        for _ in 0..k {
            match op {
                BinOp::Shl => {
                    self.emit("    BCF STATUS, 0 ; C".to_string());
                    for i in 0..n {
                        let f = dst + u16::from(i);
                        self.emit_bank_select(f);
                        self.emit(format!("    RLF {}, F", self.fop(f)));
                    }
                }
                BinOp::LShr => {
                    self.emit("    BCF STATUS, 0 ; C".to_string());
                    for i in (0..n).rev() {
                        let f = dst + u16::from(i);
                        self.emit_bank_select(f);
                        self.emit(format!("    RRF {}, F", self.fop(f)));
                    }
                }
                BinOp::AShr => {
                    let hi = dst + u16::from(n - 1);
                    self.emit_bank_select(hi);
                    self.emit(format!("    BTFSC {}, 7", self.fop(hi)));
                    self.emit("    BSF STATUS, 0 ; C".to_string());
                    self.emit(format!("    BTFSS {}, 7", self.fop(hi)));
                    self.emit("    BCF STATUS, 0 ; C".to_string());
                    for i in (0..n).rev() {
                        let f = dst + u16::from(i);
                        self.emit_bank_select(f);
                        self.emit(format!("    RRF {}, F", self.fop(f)));
                    }
                }
                _ => unreachable!("isel: emit_shift_const takes a shift op"),
            }
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
    /// FSR first and read INDF; a const (flash) base reads via
    /// `CALL __read_<name>` (the RETLW table leaves the byte in W).
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), k, terms) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        self.emit_const_read(&name, k, &terms, byte_off);
                        return;
                    }
                }
            }
            Val::Global(g) => {
                if self.global_is_const(g) {
                    // A const global used directly as a pointer (memcpy
                    // src): constant byte index, no terms.
                    self.emit_const_read(g, 0, &[], byte_off);
                    return;
                }
            }
            Val::Const(_) => panic!("isel: load through a constant pointer"),
        }
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVF {}, W", self.fop(a)))
            }
            Addr::Indirect => {
                // The pointer was just fully loaded into FSR (bank bit and
                // offset together, D-2 item 2), so FSR<5> is never stale at
                // an INDF touch: no reassertion here, or a bank-1 pointer
                // (0x30-0x3F) would be silently redirected to bank 0.
                self.emit("    MOVF INDF, W".to_string())
            }
        }
    }
    /// `W = k + byte_off + terms`: the RETLW-table index for a const
    /// (flash) read. Same fold as `emit_fsr_to`'s W computation, minus the
    /// FSR store; the reader adds the table base itself.
    fn emit_const_index_w(&mut self, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let lit = k.wrapping_add(byte_off);
        match terms {
            [] => {
                self.emit(format!("    MOVLW 0x{lit:02X}"));
            }
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit_bank_select(a);
                self.emit(format!("    MOVF {}, W", self.fop(a)));
                self.emit_add_w_const(lit);
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF {}, W", self.fop(self.scratch)));
                self.emit_add_w_const(lit);
            }
        }
    }

    /// `W = flash[name][k + byte_off + terms]` via the table's `__read_`
    /// entry: index to W, then a managed CALL (PA0 to the table's page,
    /// restore to the caller's). BSF/BCF touch only PA0, and CALL
    /// preserves W, so the byte arrives in W with no park.
    fn emit_const_read(&mut self, name: &str, k: u8, terms: &[(u8, String)], byte_off: u8) {
        self.emit_const_index_w(k, terms, byte_off);
        let reader = format!("__read_{name}");
        self.emit_paged_call(
            &reader,
            Pa0Page::Table(name.to_string()),
            Pa0Page::Func(self.cur_func.to_string()),
        );
    }

    /// `RAM[ptr + byte_off] = W`: the store side of a byte access. W is
    /// staged in common RAM first: the FSR setup clobbers W, and a banked
    /// value's reassert would clobber FSR (silent wrong-bank write).
    /// `store_tmp` is dead between instructions and untouched by the FSR
    /// setup paths; MOVWF leaves W intact, so the direct arm needs no
    /// reload.
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        self.emit(format!("    MOVWF {}", self.fop(self.store_tmp)));
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF {}", self.fop(a)))
            }
            Addr::Indirect => {
                self.emit(format!("    MOVF {}, W", self.fop(self.store_tmp)));
                self.emit("    MOVWF INDF".to_string())
            }
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`. The value byte is
    /// staged in common RAM BEFORE the FSR setup: a banked value's load
    /// reasserts FSR<5>, which would clobber the just-set pointer FSR
    /// before the INDF store (silent wrong-bank write). `store_tmp` is
    /// dead between instructions and untouched by the FSR setup paths;
    /// MOVWF leaves W intact, so the direct arm needs no reload.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        self.emit_load_byte(val, byte_off);
        self.emit(format!("    MOVWF {}", self.fop(self.store_tmp)));
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_bank_select(a);
                self.emit(format!("    MOVWF {}", self.fop(a)));
            }
            Addr::Indirect => {
                self.emit(format!("    MOVF {}, W", self.fop(self.store_tmp)));
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
                self.emit(format!("    MOVF {}, W", self.fop(a)));
                self.emit_add_w_const(lit as u8);
                self.emit("    MOVWF FSR".to_string());
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF {}, W", self.fop(self.scratch)));
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
            self.emit(format!("    MOVF {}, W", self.fop(slot_addr)));
            self.emit_add_w_const(kk as u8);
            self.emit("    MOVWF FSR".to_string());
        } else {
            self.emit_accum_terms(terms);
            self.emit_bank_select(slot_addr);
            self.emit(format!("    MOVF {}, W", self.fop(slot_addr)));
            self.emit(format!("    ADDWF {}, W", self.fop(self.scratch)));
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
                self.emit(format!("    MOVF {}, W", self.fop(a)));
                self.emit(format!("    ADDWF {}, W", self.fop(self.scratch)));
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
            self.emit(format!("    MOVF {}, W", self.fop(fp)));
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit_bank_select(fp + 1);
            self.emit(format!("    MOVF {}, W", self.fop(fp + 1)));
            self.emit(format!("    XORLW HIGH({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit_call_args(cand, args);
            self.emit_paged_call(
                cand,
                Pa0Page::Func(cand.clone()),
                Pa0Page::Func(self.cur_func.to_string()),
            );
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
                let rv = self.retval_lo + u16::from(i);
                self.emit_bank_select(rv);
                self.emit(format!("    MOVF {}, W", self.fop(rv)));
                self.emit_w_store(da + u16::from(i));
            }
        }
    }

    /// `CALL {target}` with PA0 managed: a set-bit placeholder (fixup:
    /// the target's page), the CALL, a restore-bit placeholder (fixup:
    /// the caller's page). Uniform 3 words so emission sizes stay
    /// page-independent; layout patches the bits in place.
    fn emit_paged_call(&mut self, target: &str, set: Pa0Page, restore: Pa0Page) {
        self.emit("    BCF STATUS, 5".to_string());
        self.fixups.push((self.out.len() - 1, set));
        self.emit(format!("    CALL {target}"));
        self.emit("    BCF STATUS, 5".to_string());
        self.fixups.push((self.out.len() - 1, restore));
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
        self.emit_paged_call(
            func,
            Pa0Page::Func(func.to_string()),
            Pa0Page::Func(self.cur_func.to_string()),
        );
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                let rv = self.retval_lo + u16::from(i);
                self.emit_bank_select(rv);
                self.emit(format!("    MOVF {}, W", self.fop(rv)));
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
                    self.emit(format!("    MOVWF {}", self.fop(pa + u16::from(b))));
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
                self.emit(format!("    MOVWF {}", self.fop(pa)));
                self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                self.emit_bank_select(pa + 1);
                self.emit(format!("    MOVWF {}", self.fop(pa + 1)));
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
                            self.emit(format!("    MOVWF {}", self.fop(pa)));
                            self.emit(format!("    MOVLW HIGH({g})"));
                            self.emit_bank_select(pa + 1);
                            self.emit(format!("    MOVWF {}", self.fop(pa + 1)));
                        } else {
                            if self.global_is_const(g) {
                                panic!("isel: const global @{g} too large for RAM copy");
                            }
                            let addr = self.global_addr(g);
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                            self.emit_bank_select(pa);
                            self.emit(format!("    MOVWF {}", self.fop(pa)));
                            self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                            self.emit_bank_select(pa + 1);
                            self.emit(format!("    MOVWF {}", self.fop(pa + 1)));
                        }
                    }
                    Val::Const(c) => {
                        assert_eq!(*c, 0, "isel: non-zero const ptr not supported");
                        self.emit_bank_select(pa);
                        self.emit(format!("    CLRF {}", self.fop(pa)));
                        self.emit_bank_select(pa + 1);
                        self.emit(format!("    CLRF {}", self.fop(pa + 1)));
                    }
                    Val::Reg(r) => {
                        // A runtime pointer value: copy its two address bytes.
                        let sa = self.slot_addr(self.cur_func, r).direct();
                        self.emit_bank_select(sa);
                        self.emit(format!("    MOVF {}, W", self.fop(sa)));
                        self.emit_bank_select(pa);
                        self.emit(format!("    MOVWF {}", self.fop(pa)));
                        self.emit_bank_select(sa + 1);
                        self.emit(format!("    MOVF {}, W", self.fop(sa + 1)));
                        self.emit_bank_select(pa + 1);
                        self.emit(format!("    MOVWF {}", self.fop(pa + 1)));
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
                    if self.global_is_const(g) {
                        // A flash const with a constant index: one RETLW
                        // call per byte (i16 reads low then high).
                        for k in 0..l.ty.bytes() {
                            self.emit_const_read(g, 0, &[], k);
                            self.emit_w_store(dst + u16::from(k));
                        }
                    } else {
                        let src = self.global_addr(g);
                        for k in 0..l.ty.bytes() {
                            self.emit_bank_select(src + u16::from(k));
                            self.emit(format!("    MOVF {}, W", self.fop(src + u16::from(k))));
                            self.emit_w_store(dst + u16::from(k));
                        }
                    }
                } else if l.ptr.starts_with("0x") {
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit_bank_select(base + u16::from(k));
                        self.emit(format!("    MOVF {}, W", self.fop(base + u16::from(k))));
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
                        self.emit(format!("    MOVWF {}", self.fop(base + u16::from(k))));
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
                                self.emit(format!("    MOVF {}, W", self.fop(aa)));
                                self.emit_add_w_const(kb);
                                self.emit_w_store(da);
                            }
                            _ => {
                                let (aa, bb) =
                                    (self.val_addr(a).direct(), self.val_addr(b_op).direct());
                                self.emit_bank_select(bb);
                                self.emit(format!("    MOVF {}, W", self.fop(bb)));
                                self.emit_bank_select(aa);
                                self.emit(format!("    ADDWF {}, W", self.fop(aa)));
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
                    (BinOp::Add, Ty::I32) => self.emit_add32(&b.a, &b.b, da),
                    (BinOp::Sub, Ty::I32) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 4);
                        } else {
                            self.emit_sub32(&b.a, &b.b, da);
                        }
                    }
                    (BinOp::And, Ty::I32) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW")
                    }
                    (BinOp::Or, Ty::I32) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW")
                    }
                    (BinOp::Xor, Ty::I32) => {
                        self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW")
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
                        match &b.b {
                            Val::Const(k) => self.emit_shift_const(b.op, &b.a, b.ty, *k, da),
                            other => panic!(
                                "isel: variable-count {:?} shift reached isel (count {other:?}); legalize must rewrite it to a routine call",
                                b.op
                            ),
                        }
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
                    self.emit(format!("    CLRF {}", self.fop(da + u16::from(i))));
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
                self.emit(format!("    BTFSS {}, 7", self.fop(a + u16::from(src_hi))));
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
                    self.emit(format!("    ANDWF {}, F", self.fop(da)));
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
                    self.emit(format!(
                        "    MOVWF {}",
                        self.fop(self.retval_lo + u16::from(i))
                    ));
                }
                self.emit("    RETLW 0x00".to_string());
            }
            _ => panic!("isel: unsupported terminator for P2"),
        }
    }
    /// Copy a multi-byte result into the fixed retval slots: `emit_call`
    /// on the caller side reads them after CALL. Selects are unconditional
    /// (no-ops on common RAM) per D-2.
    fn store_retval(&mut self, src: u16, bytes: u8) {
        for i in 0..bytes {
            let (s, d) = (src + u16::from(i), self.retval_lo + u16::from(i));
            self.emit_bank_select(s);
            self.emit(format!("    MOVF {}, W", self.fop(s)));
            self.emit_bank_select(d);
            self.emit(format!("    MOVWF {}", self.fop(d)));
        }
    }

    /// Shared AN526/divmod loop helpers for the routine recipes below.
    /// One 16-iteration AN526 shift-add chunk over 4-byte r/t: test the
    /// multiplier LSB, r += t with the INCFSZ carry idiom, t <<= 1 with
    /// wraparound (high bits drop, i32 mul wraps), bk >>= 1. All files
    /// share one `__scr` slot (single-bank by placement), and the
    /// BTFSC/INCFSZ/ADDWF chains are atomic: selects never split them.
    fn emit_mul32_loop(
        &mut self,
        l_loop: String,
        l_skip: String,
        bk_lo: u16,
        bk_hi: u16,
        cnt: u16,
        r: [u16; 4],
        t: [u16; 4],
    ) {
        self.emit(format!("{l_loop}:"));
        self.emit_bank_select(bk_lo);
        self.emit(format!("    BTFSS {}, 0", self.fop(bk_lo))); // test multiplier LSB
        self.emit(format!("    GOTO {l_skip}"));
        self.emit_bank_select(t[0]);
        self.emit(format!("    MOVF {}, W", self.fop(t[0])));
        self.emit_bank_select(r[0]);
        self.emit(format!("    ADDWF {}, F", self.fop(r[0])));
        for i in 1..4 {
            self.emit_bank_select(t[i]);
            self.emit(format!("    MOVF {}, W", self.fop(t[i])));
            // Atomic carry chain like __mul_u16: FSR already selects
            // the shared __scr bank, no reassertion inside (a skip
            // would land on the select and run the add with W = 0).
            self.emit("    BTFSC STATUS, 0 ; C".to_string());
            self.emit(format!("    INCFSZ {}, W", self.fop(t[i])));
            self.emit(format!("    ADDWF {}, F", self.fop(r[i])));
        }
        self.emit(format!("{l_skip}:"));
        self.emit("    BCF STATUS, 0 ; C".to_string());
        for t_i in t {
            self.emit_bank_select(t_i);
            self.emit(format!("    RLF {}, F", self.fop(t_i))); // t <<= 1 (wrapping)
        }
        self.emit("    BCF STATUS, 0 ; C".to_string());
        self.emit_bank_select(bk_hi);
        self.emit(format!("    RRF {}, F", self.fop(bk_hi)));
        self.emit_bank_select(bk_lo);
        self.emit(format!("    RRF {}, F", self.fop(bk_lo))); // bk >>= 1
        self.emit_bank_select(cnt);
        self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
        self.emit(format!("    GOTO {l_loop}"));
    }

    /// The 32-iteration restoring-division loop for i32 divmod: `num`
    /// shifts left (the quotient builds in its vacated bits), `rem`
    /// accumulates, `den` holds the denominator copy, `cnt` counts 32.
    /// Borrow chains are register-direct (no ADDLW); all files share one
    /// `__scr` slot and every skip chain is atomic.
    fn emit_divmod32(&mut self, num: u16, scr: u16) {
        let (rem0, den0, cnt) = (scr, scr + 4, scr + 8);
        let l_loop = self.fresh_label();
        let l_restore = self.fresh_label();
        let l_next = self.fresh_label();
        for i in 0..4 {
            let r = rem0 + i as u16;
            self.emit_bank_select(r);
            self.emit(format!("    CLRF {}", self.fop(r)));
        }
        self.emit("    MOVLW 0x20".to_string());
        self.emit_bank_select(cnt);
        self.emit(format!("    MOVWF {}", self.fop(cnt)));
        self.emit(format!("{l_loop}:"));
        self.emit("    BCF STATUS, 0 ; C".to_string());
        for i in 0..4 {
            let f = num + i as u16;
            self.emit_bank_select(f);
            self.emit(format!("    RLF {}, F", self.fop(f)));
        }
        for i in 0..4 {
            let r = rem0 + i as u16;
            self.emit_bank_select(r);
            self.emit(format!("    RLF {}, F", self.fop(r)));
        }
        // rem -= den across 4 bytes (INCFSZ wrap-correct borrow folds);
        // C = (rem >= den) after the last byte.
        for i in 0..4 {
            let (d, r) = (den0 + i as u16, rem0 + i as u16);
            self.emit_bank_select(d);
            self.emit(format!("    MOVF {}, W", self.fop(d)));
            if i == 0 {
                self.emit_bank_select(r);
                self.emit(format!("    SUBWF {}, F", self.fop(r)));
            } else {
                // Atomic borrow chain: den and rem share __scr's bank
                // (FSR set by the MOVF), no reassertion inside.
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(d)));
                self.emit(format!("    SUBWF {}, F", self.fop(r)));
            }
        }
        self.emit("    BTFSS STATUS, 0 ; C".to_string());
        self.emit(format!("    GOTO {l_restore}"));
        self.emit_bank_select(num);
        self.emit(format!("    BSF {}, 0", self.fop(num)));
        self.emit(format!("    GOTO {l_next}"));
        self.emit(format!("{l_restore}:"));
        // rem += den (the exact add-back restore, carry folds).
        for i in 0..4 {
            let (d, r) = (den0 + i as u16, rem0 + i as u16);
            self.emit_bank_select(d);
            self.emit(format!("    MOVF {}, W", self.fop(d)));
            if i == 0 {
                self.emit_bank_select(r);
                self.emit(format!("    ADDWF {}, F", self.fop(r)));
            } else {
                // Atomic carry chain, same __scr bank as above.
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(d)));
                self.emit(format!("    ADDWF {}, F", self.fop(r)));
            }
        }
        self.emit(format!("{l_next}:"));
        self.emit_bank_select(cnt);
        self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
        self.emit(format!("    GOTO {l_loop}"));
    }

    /// In-place two's-complement negates for the signed wrappers.
    /// Two's-complement negate of a 16-bit value in place. The select
    /// for the high byte sits before the BTFSC, never between it and
    /// its INCF target.
    fn neg16_in_place(&mut self, addr: u16) {
        self.emit_bank_select(addr);
        self.emit(format!("    COMF {}, F", self.fop(addr)));
        self.emit_bank_select(addr + 1);
        self.emit(format!("    COMF {}, F", self.fop(addr + 1)));
        self.emit_bank_select(addr);
        self.emit(format!("    INCF {}, F", self.fop(addr)));
        self.emit_bank_select(addr + 1);
        self.emit("    BTFSC STATUS, 2 ; Z".to_string());
        self.emit(format!("    INCF {}, F", self.fop(addr + 1)));
    }

    /// Two's-complement negate of a 32-bit value in place (the INCF
    /// carry propagates byte-by-byte through the Z chain, cascading
    /// skips included).
    fn neg32_in_place(&mut self, addr: u16) {
        for i in 0..4 {
            let f = addr + i as u16;
            self.emit_bank_select(f);
            self.emit(format!("    COMF {}, F", self.fop(f)));
        }
        self.emit_bank_select(addr);
        self.emit(format!("    INCF {}, F", self.fop(addr)));
        for i in 1..4 {
            let f = addr + i as u16;
            self.emit_bank_select(f);
            self.emit("    BTFSC STATUS, 2 ; Z".to_string());
            self.emit(format!("    INCF {}, F", self.fop(f)));
        }
    }
    /// Variable-count shifts over 1/2/4 bytes, in place in the `val`
    /// param slot. The count arrives unmasked; masking to width-1 keeps
    /// the loop bounded (LLVM counts >= width are poison). ashr sets C
    /// from the sign bit before each rrf. Selects never split a skip
    /// chain (the ashr sign pair selects before the BTFSC).
    fn emit_shift_body(&mut self, bytes: u16, op: BinOp, scr: u16) {
        let name = self.cur_func.to_string();
        let val = self.slot_addr(&name, "val").direct();
        let cnt = self.slot_addr(&name, "cnt").direct();
        let hi = val + bytes - 1;
        let mask: u8 = match bytes {
            1 => 0x07,
            2 => 0x0F,
            4 => 0x1F,
            _ => unreachable!("isel: shift body width"),
        }; // width - 1
        self.emit_bank_select(cnt);
        self.emit(format!("    MOVF {}, W", self.fop(cnt)));
        self.emit(format!("    ANDLW 0x{mask:02X}")); // count & (width-1)
        self.emit_bank_select(scr);
        self.emit(format!("    MOVWF {}", self.fop(scr))); // __scr::cnt@0 = masked count
        if bytes == 2 {
            // The high byte of the masked 2-byte cnt slot stays 0: the
            // masked count is < 16, so the loop counter lives in the low
            // byte. Clear it once so a stale high byte from an earlier
            // call cannot be misread as part of the count.
            self.emit_bank_select(scr + 1);
            self.emit(format!("    CLRF {}", self.fop(scr + 1)));
        }
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        // count == 0 shifts nothing: skip the loop entirely (a bare
        // DECFSZ-at-bottom loop would run once on a zero counter).
        self.emit_bank_select(scr);
        self.emit(format!("    MOVF {}, F", self.fop(scr))); // Z = (cnt == 0)
        self.emit("    BTFSC STATUS, 2 ; Z".to_string()); // skip the GOTO when cnt != 0
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_loop}:"));
        match op {
            BinOp::Shl => {
                self.emit("    BCF STATUS, 0 ; C".to_string());
                for i in 0..bytes {
                    let f = val + u16::from(i);
                    self.emit_bank_select(f);
                    self.emit(format!("    RLF {}, F", self.fop(f)));
                }
            }
            BinOp::LShr => {
                self.emit("    BCF STATUS, 0 ; C".to_string());
                for i in (0..bytes).rev() {
                    let f = val + u16::from(i);
                    self.emit_bank_select(f);
                    self.emit(format!("    RRF {}, F", self.fop(f)));
                }
            }
            BinOp::AShr => {
                self.emit_bank_select(hi);
                self.emit(format!("    BTFSC {}, 7", self.fop(hi)));
                self.emit("    BSF STATUS, 0 ; C".to_string());
                self.emit(format!("    BTFSS {}, 7", self.fop(hi)));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                for i in (0..bytes).rev() {
                    let f = val + u16::from(i);
                    self.emit_bank_select(f);
                    self.emit(format!("    RRF {}, F", self.fop(f)));
                }
            }
            _ => unreachable!("isel: shift body takes a shift op"),
        }
        self.emit_bank_select(scr);
        self.emit(format!("    DECFSZ {}, F", self.fop(scr)));
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_done}:"));
        self.store_retval(val, bytes as u8);
        self.emit("    RETLW 0x00".to_string());
    }

    /// A legalize-injected runtime routine body (P6): args arrive in the
    /// routine's `{func}::{param}` slots (copied by `emit_call`), the
    /// result goes to the fixed retval slots, working state lives in
    /// `{func}::__scr`. The byte logic mirrors classic `isel`'s recipes;
    /// every file access carries the D-2 bank select (frames may span
    /// banks), literal-adds use the D-6 idiom, returns are `RETLW 0`,
    /// and skip chains stay inside one `__scr` value (atomic, never
    /// split by a select).
    fn emit_routine(&mut self) {
        let name = self.cur_func.to_string();
        let recipe = routine_recipe(&name)
            .unwrap_or_else(|| panic!("isel: @{name} is not a runtime routine"));
        let scr = self.slot_addr(&name, "__scr").direct();
        self.emit(format!("{name}:"));
        match recipe {
            // 8x8 -> 16 shift-add (AN526): t = a shifted left one bit per
            // multiplier bit; for each set bit of bk, r += t. Store the low
            // byte of the product (the i8 result).
            "__mul_u8" => {
                let a = self.slot_addr(&name, "a").direct();
                let b = self.slot_addr(&name, "b").direct();
                let (bk, cnt, r_lo, r_hi, t_lo, t_hi) =
                    (scr, scr + 1, scr + 2, scr + 3, scr + 4, scr + 5);
                let l_loop = self.fresh_label();
                let l_skip = self.fresh_label();
                for r in [r_lo, r_hi, t_lo, t_hi] {
                    self.emit_bank_select(r);
                    self.emit(format!("    CLRF {}", self.fop(r)));
                }
                self.emit_bank_select(a);
                self.emit(format!("    MOVF {}, W", self.fop(a)));
                self.emit_bank_select(t_lo);
                self.emit(format!("    MOVWF {}", self.fop(t_lo))); // t = a
                self.emit_bank_select(b);
                self.emit(format!("    MOVF {}, W", self.fop(b)));
                self.emit_bank_select(bk);
                self.emit(format!("    MOVWF {}", self.fop(bk))); // bk = b
                self.emit("    MOVLW 0x08".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt))); // cnt = 8
                self.emit(format!("{l_loop}:"));
                self.emit_bank_select(bk);
                self.emit(format!("    BTFSS {}, 0", self.fop(bk))); // test multiplier LSB
                self.emit(format!("    GOTO {l_skip}"));
                self.emit_bank_select(t_lo);
                self.emit(format!("    MOVF {}, W", self.fop(t_lo)));
                self.emit_bank_select(r_lo);
                self.emit(format!("    ADDWF {}, F", self.fop(r_lo)));
                self.emit_bank_select(t_hi);
                self.emit(format!("    MOVF {}, W", self.fop(t_hi)));
                // The BTFSC/INCFSZ/ADDWF carry chain is atomic: no select
                // may split it (a skip would land on the select). FSR
                // already selects t_hi's bank from the MOVF above, and
                // r_hi shares __scr's single-bank slot.
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(t_hi))); // t_hi + carry; skip if wrapped
                self.emit(format!("    ADDWF {}, F", self.fop(r_hi)));
                self.emit(format!("{l_skip}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(t_lo);
                self.emit(format!("    RLF {}, F", self.fop(t_lo)));
                self.emit_bank_select(t_hi);
                self.emit(format!("    RLF {}, F", self.fop(t_hi))); // t <<= 1
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(bk);
                self.emit(format!("    RRF {}, F", self.fop(bk))); // bk >>= 1
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                self.store_retval(r_lo, 2);
                self.emit("    RETLW 0x00".to_string());
            }
            // 8/8 restoring division (8 iterations): num <<= 1 (C = old
            // MSB); rem = (rem << 1) | C; if rem >= den set the quotient
            // bit else restore (add den back). rem is 2 bytes: the 8-bit
            // rem shift can carry. Borrow/carry folds use the D-6
            // carry helpers (skip-safe units) instead of ADDLW.
            "__udiv_u8" | "__urem_u8" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                let (rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2);
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                self.emit_bank_select(rem_lo);
                self.emit(format!("    CLRF {}", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    CLRF {}", self.fop(rem_hi)));
                self.emit("    MOVLW 0x08".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt)));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(num);
                self.emit(format!("    RLF {}, F", self.fop(num)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    RLF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    RLF {}, F", self.fop(rem_hi)));
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_lo)));
                self.emit("    MOVLW 0x00".to_string());
                self.emit_add_w_borrow(); // W = borrow
                self.emit_bank_select(rem_hi);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_hi))); // C = (rem >= den)
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit_bank_select(num);
                self.emit(format!("    BSF {}, 0", self.fop(num)));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_lo)));
                self.emit("    MOVLW 0x00".to_string());
                self.emit_add_w_carry(); // W = carry
                self.emit_bank_select(rem_hi);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_hi)));
                self.emit(format!("{l_next}:"));
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__udiv_u8" {
                    self.store_retval(num, 1);
                } else {
                    self.store_retval(rem_lo, 1);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // 16x16 -> 32 shift-add, 16 iterations: t = a (32-bit, shifted
            // left), for each set bit of bk, r += t across all 4 bytes with
            // the incfsz carry idiom. Store the low 16 bits (the i16 result).
            "__mul_u16" => {
                let a = self.slot_addr(&name, "a").direct();
                let b = self.slot_addr(&name, "b").direct();
                let (bk_lo, bk_hi, cnt) = (scr, scr + 1, scr + 2);
                let (r0, r1, r2, r3) = (scr + 3, scr + 4, scr + 5, scr + 6);
                let (t0, t1, t2, t3) = (scr + 7, scr + 8, scr + 9, scr + 10);
                let l_loop = self.fresh_label();
                let l_skip = self.fresh_label();
                for r in [r0, r1, r2, r3] {
                    self.emit_bank_select(r);
                    self.emit(format!("    CLRF {}", self.fop(r)));
                }
                for t in [t0, t1, t2, t3] {
                    self.emit_bank_select(t);
                    self.emit(format!("    CLRF {}", self.fop(t)));
                }
                self.emit_bank_select(a);
                self.emit(format!("    MOVF {}, W", self.fop(a)));
                self.emit_bank_select(t0);
                self.emit(format!("    MOVWF {}", self.fop(t0)));
                self.emit_bank_select(a + 1);
                self.emit(format!("    MOVF {}, W", self.fop(a + 1)));
                self.emit_bank_select(t1);
                self.emit(format!("    MOVWF {}", self.fop(t1))); // t = a (32-bit, low 16)
                self.emit_bank_select(b);
                self.emit(format!("    MOVF {}, W", self.fop(b)));
                self.emit_bank_select(bk_lo);
                self.emit(format!("    MOVWF {}", self.fop(bk_lo)));
                self.emit_bank_select(b + 1);
                self.emit(format!("    MOVF {}, W", self.fop(b + 1)));
                self.emit_bank_select(bk_hi);
                self.emit(format!("    MOVWF {}", self.fop(bk_hi))); // bk = b
                self.emit("    MOVLW 0x10".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt))); // cnt = 16
                self.emit(format!("{l_loop}:"));
                self.emit_bank_select(bk_lo);
                self.emit(format!("    BTFSS {}, 0", self.fop(bk_lo))); // test multiplier LSB
                self.emit(format!("    GOTO {l_skip}"));
                self.emit_bank_select(t0);
                self.emit(format!("    MOVF {}, W", self.fop(t0)));
                self.emit_bank_select(r0);
                self.emit(format!("    ADDWF {}, F", self.fop(r0)));
                for (ti, ri) in [(t1, r1), (t2, r2), (t3, r3)] {
                    self.emit_bank_select(ti);
                    self.emit(format!("    MOVF {}, W", self.fop(ti)));
                    // Atomic carry chain (see __mul_u8): FSR already
                    // selects the shared __scr bank, no reassertion inside.
                    self.emit("    BTFSC STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ {}, W", self.fop(ti)));
                    self.emit(format!("    ADDWF {}, F", self.fop(ri)));
                }
                self.emit(format!("{l_skip}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                for t in [t0, t1, t2, t3] {
                    self.emit_bank_select(t);
                    self.emit(format!("    RLF {}, F", self.fop(t))); // t <<= 1
                }
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(bk_hi);
                self.emit(format!("    RRF {}, F", self.fop(bk_hi)));
                self.emit_bank_select(bk_lo);
                self.emit(format!("    RRF {}, F", self.fop(bk_lo))); // bk >>= 1
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                self.store_retval(r0, 2);
                self.emit("    RETLW 0x00".to_string());
            }
            // 32x32 -> 32 shift-add as two 16-iteration chunks (bk reloaded
            // from b's high half between chunks): the low 32 bits of the
            // full product (the i32 result).
            "__mul_u32" => {
                let a = self.slot_addr(&name, "a").direct();
                let b = self.slot_addr(&name, "b").direct();
                let (bk_lo, bk_hi, cnt) = (scr, scr + 1, scr + 2);
                let r = [scr + 3, scr + 4, scr + 5, scr + 6];
                let t = [scr + 7, scr + 8, scr + 9, scr + 10];
                for i in 0..4 {
                    self.emit_bank_select(r[i]);
                    self.emit(format!("    CLRF {}", self.fop(r[i])));
                    self.emit_bank_select(t[i]);
                    self.emit(format!("    CLRF {}", self.fop(t[i])));
                }
                for i in 0..4u16 {
                    self.emit_bank_select(a + i);
                    self.emit(format!("    MOVF {}, W", self.fop(a + i)));
                    self.emit_bank_select(t[usize::from(i)]);
                    self.emit(format!("    MOVWF {}", self.fop(t[usize::from(i)])));
                    // t = a (32-bit)
                }
                self.emit_bank_select(b);
                self.emit(format!("    MOVF {}, W", self.fop(b)));
                self.emit_bank_select(bk_lo);
                self.emit(format!("    MOVWF {}", self.fop(bk_lo)));
                self.emit_bank_select(b + 1);
                self.emit(format!("    MOVF {}, W", self.fop(b + 1)));
                self.emit_bank_select(bk_hi);
                self.emit(format!("    MOVWF {}", self.fop(bk_hi))); // bk = b low 16
                self.emit("    MOVLW 0x10".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt))); // cnt = 16
                let l_loop1 = self.fresh_label();
                let l_skip1 = self.fresh_label();
                self.emit_mul32_loop(l_loop1, l_skip1, bk_lo, bk_hi, cnt, r, t);
                // Reload bk from b's high half for the second 16 iterations.
                self.emit_bank_select(b + 2);
                self.emit(format!("    MOVF {}, W", self.fop(b + 2)));
                self.emit_bank_select(bk_lo);
                self.emit(format!("    MOVWF {}", self.fop(bk_lo)));
                self.emit_bank_select(b + 3);
                self.emit(format!("    MOVF {}, W", self.fop(b + 3)));
                self.emit_bank_select(bk_hi);
                self.emit(format!("    MOVWF {}", self.fop(bk_hi)));
                self.emit("    MOVLW 0x10".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt)));
                let l_loop2 = self.fresh_label();
                let l_skip2 = self.fresh_label();
                self.emit_mul32_loop(l_loop2, l_skip2, bk_lo, bk_hi, cnt, r, t);
                self.store_retval(scr + 3, 4);
                self.emit("    RETLW 0x00".to_string());
            }
            // 32/32 restoring division (32 iterations): den copied into
            // __scr, the shared loop runs, udiv keeps num (quotient),
            // urem keeps rem.
            "__udiv_u32" | "__urem_u32" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                for i in 0..4 {
                    self.emit_bank_select(den + i as u16);
                    self.emit(format!("    MOVF {}, W", self.fop(den + i as u16)));
                    self.emit_bank_select(scr + 4 + i as u16);
                    self.emit(format!("    MOVWF {}", self.fop(scr + 4 + i as u16))); // den copy
                }
                self.emit_divmod32(num, scr);
                if recipe == "__udiv_u32" {
                    self.store_retval(num, 4);
                } else {
                    self.store_retval(scr, 4);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // 16/16 restoring division (16 iterations) with the
            // register-direct borrow idiom (no ADDLW): udiv keeps num,
            // urem keeps rem.
            "__udiv_u16" | "__urem_u16" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                let (rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2);
                // Denominator copy in free __scr bytes (spare@3 and
                // restore@4 per the legalize contract, unused below):
                // the borrow/restore chains must stay inside one __scr
                // value, and the param slot may sit in another bank.
                let (den_lo, den_hi) = (scr + 3, scr + 4);
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVWF {}", self.fop(den_lo)));
                self.emit_bank_select(den + 1);
                self.emit(format!("    MOVF {}, W", self.fop(den + 1)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVWF {}", self.fop(den_hi)));
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                self.emit_bank_select(rem_lo);
                self.emit(format!("    CLRF {}", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    CLRF {}", self.fop(rem_hi)));
                self.emit("    MOVLW 0x10".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt)));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(num);
                self.emit(format!("    RLF {}, F", self.fop(num)));
                self.emit_bank_select(num + 1);
                self.emit(format!("    RLF {}, F", self.fop(num + 1)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    RLF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    RLF {}, F", self.fop(rem_hi)));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVF {}, W", self.fop(den_lo)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVF {}, W", self.fop(den_hi)));
                // Atomic borrow chain: den copy and rem share __scr's
                // bank (FSR set by the MOVF), no reassertion inside.
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(den_hi)));
                self.emit(format!("    SUBWF {}, F", self.fop(rem_hi)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit_bank_select(num);
                self.emit(format!("    BSF {}, 0", self.fop(num)));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVF {}, W", self.fop(den_lo)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVF {}, W", self.fop(den_hi)));
                // Atomic carry chain, same __scr bank as above.
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(den_hi)));
                self.emit(format!("    ADDWF {}, F", self.fop(rem_hi)));
                self.emit(format!("{l_next}:"));
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__udiv_u16" {
                    self.store_retval(num, 2);
                } else {
                    self.store_retval(rem_lo, 2);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // Signed 8-bit wrappers: abs both operands in place in the
            // param slots (unsigned abs, INT_MIN safe), run the unsigned
            // divmod, negate the quotient if the signs differed (bit0) /
            // the remainder if the dividend was negative (bit1).
            "__sdiv_i8" | "__srem_i8" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                let (flags, rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2, scr + 3);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit_bank_select(flags);
                self.emit(format!("    CLRF {}", self.fop(flags)));
                self.emit_bank_select(num);
                self.emit(format!("    BTFSS {}, 7", self.fop(num)));
                self.emit(format!("    GOTO {l_den}"));
                self.emit_bank_select(flags);
                self.emit(format!("    BSF {}, 1", self.fop(flags))); // remainder sign follows dividend
                self.emit(format!("    BSF {}, 0", self.fop(flags))); // quotient negate: num<0
                self.emit_bank_select(num);
                self.emit(format!("    COMF {}, F", self.fop(num)));
                self.emit(format!("    INCF {}, F", self.fop(num))); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit_bank_select(den);
                self.emit(format!("    BTFSS {}, 7", self.fop(den)));
                self.emit(format!("    GOTO {l_go}"));
                self.emit_bank_select(den);
                self.emit(format!("    COMF {}, F", self.fop(den)));
                self.emit(format!("    INCF {}, F", self.fop(den))); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit_bank_select(flags);
                self.emit(format!("    XORWF {}, F", self.fop(flags))); // bit0 ^= den<0
                self.emit(format!("{l_go}:"));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    CLRF {}", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    CLRF {}", self.fop(rem_hi)));
                self.emit("    MOVLW 0x08".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt)));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(num);
                self.emit(format!("    RLF {}, F", self.fop(num)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    RLF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    RLF {}, F", self.fop(rem_hi)));
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_lo)));
                self.emit("    MOVLW 0x00".to_string());
                self.emit_add_w_borrow(); // W = borrow
                self.emit_bank_select(rem_hi);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_hi)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit_bank_select(num);
                self.emit(format!("    BSF {}, 0", self.fop(num)));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_lo)));
                self.emit("    MOVLW 0x00".to_string());
                self.emit_add_w_carry(); // W = carry
                self.emit_bank_select(rem_hi);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_hi)));
                self.emit(format!("{l_next}:"));
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__sdiv_i8" {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 0", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.emit_bank_select(num);
                    self.emit(format!("    COMF {}, F", self.fop(num)));
                    self.emit(format!("    INCF {}, F", self.fop(num)));
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 1);
                } else {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 1", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.emit_bank_select(rem_lo);
                    self.emit(format!("    COMF {}, F", self.fop(rem_lo)));
                    self.emit(format!("    INCF {}, F", self.fop(rem_lo)));
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem_lo, 1);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // Signed 16-bit wrappers: same structure, 16-bit abs/negate
            // and the 16-bit divmod with the register-direct borrow idiom.
            "__sdiv_i16" | "__srem_i16" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                let (flags, rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2, scr + 3);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit_bank_select(flags);
                self.emit(format!("    CLRF {}", self.fop(flags)));
                self.emit_bank_select(num + 1);
                self.emit(format!("    BTFSS {}, 7", self.fop(num + 1)));
                self.emit(format!("    GOTO {l_den}"));
                self.emit_bank_select(flags);
                self.emit(format!("    BSF {}, 1", self.fop(flags))); // remainder sign follows dividend
                self.emit(format!("    BSF {}, 0", self.fop(flags))); // quotient negate: num<0
                self.neg16_in_place(num); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit_bank_select(den + 1);
                self.emit(format!("    BTFSS {}, 7", self.fop(den + 1)));
                self.emit(format!("    GOTO {l_go}"));
                self.neg16_in_place(den); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit_bank_select(flags);
                self.emit(format!("    XORWF {}, F", self.fop(flags))); // bit0 ^= den<0
                self.emit(format!("{l_go}:"));
                // |den| copy in free __scr bytes (restore@4-5 per the
                // legalize contract, unused below), taken here so the
                // abs above is included; the chains must stay inside
                // one __scr value.
                let (den_lo, den_hi) = (scr + 4, scr + 5);
                self.emit_bank_select(den);
                self.emit(format!("    MOVF {}, W", self.fop(den)));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVWF {}", self.fop(den_lo)));
                self.emit_bank_select(den + 1);
                self.emit(format!("    MOVF {}, W", self.fop(den + 1)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVWF {}", self.fop(den_hi)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    CLRF {}", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    CLRF {}", self.fop(rem_hi)));
                self.emit("    MOVLW 0x10".to_string());
                self.emit_bank_select(cnt);
                self.emit(format!("    MOVWF {}", self.fop(cnt)));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0 ; C".to_string());
                self.emit_bank_select(num);
                self.emit(format!("    RLF {}, F", self.fop(num)));
                self.emit_bank_select(num + 1);
                self.emit(format!("    RLF {}, F", self.fop(num + 1)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    RLF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(rem_hi);
                self.emit(format!("    RLF {}, F", self.fop(rem_hi)));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVF {}, W", self.fop(den_lo)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    SUBWF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVF {}, W", self.fop(den_hi)));
                // Atomic borrow chain: den copy and rem share __scr's
                // bank (FSR set by the MOVF), no reassertion inside.
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(den_hi)));
                self.emit(format!("    SUBWF {}, F", self.fop(rem_hi)));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit_bank_select(num);
                self.emit(format!("    BSF {}, 0", self.fop(num)));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit_bank_select(den_lo);
                self.emit(format!("    MOVF {}, W", self.fop(den_lo)));
                self.emit_bank_select(rem_lo);
                self.emit(format!("    ADDWF {}, F", self.fop(rem_lo)));
                self.emit_bank_select(den_hi);
                self.emit(format!("    MOVF {}, W", self.fop(den_hi)));
                // Atomic carry chain, same __scr bank as above.
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ {}, W", self.fop(den_hi)));
                self.emit(format!("    ADDWF {}, F", self.fop(rem_hi)));
                self.emit(format!("{l_next}:"));
                self.emit_bank_select(cnt);
                self.emit(format!("    DECFSZ {}, F", self.fop(cnt)));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__sdiv_i16" {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 0", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg16_in_place(num); // -quotient
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 2);
                } else {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 1", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg16_in_place(rem_lo); // -remainder
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem_lo, 2);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // Signed 32-bit wrappers: abs in place, unsigned divmod,
            // negate quotient iff signs differed / remainder iff the
            // dividend was negative.
            "__sdiv_i32" | "__srem_i32" => {
                let num = self.slot_addr(&name, "num").direct();
                let den = self.slot_addr(&name, "den").direct();
                let (rem, den_s, flags) = (scr, scr + 4, scr + 10);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit_bank_select(flags);
                self.emit(format!("    CLRF {}", self.fop(flags)));
                self.emit_bank_select(num + 3);
                self.emit(format!("    BTFSS {}, 7", self.fop(num + 3)));
                self.emit(format!("    GOTO {l_den}"));
                self.emit_bank_select(flags);
                self.emit(format!("    BSF {}, 1", self.fop(flags))); // remainder sign follows dividend
                self.emit(format!("    BSF {}, 0", self.fop(flags))); // quotient negate: num<0
                self.neg32_in_place(num); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit_bank_select(den + 3);
                self.emit(format!("    BTFSS {}, 7", self.fop(den + 3)));
                self.emit(format!("    GOTO {l_go}"));
                self.neg32_in_place(den); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit_bank_select(flags);
                self.emit(format!("    XORWF {}, F", self.fop(flags))); // bit0 ^= den<0
                self.emit(format!("{l_go}:"));
                for i in 0..4 {
                    self.emit_bank_select(den + i as u16);
                    self.emit(format!("    MOVF {}, W", self.fop(den + i as u16)));
                    self.emit_bank_select(den_s + i as u16);
                    self.emit(format!("    MOVWF {}", self.fop(den_s + i as u16))); // |den| copy
                }
                self.emit_divmod32(num, scr);
                if recipe == "__sdiv_i32" {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 0", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg32_in_place(num); // -quotient
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 4);
                } else {
                    self.emit_bank_select(flags);
                    self.emit(format!("    BTFSS {}, 1", self.fop(flags)));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg32_in_place(rem); // -remainder
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem, 4);
                }
                self.emit("    RETLW 0x00".to_string());
            }
            // Variable-count shifts: the value shifts in place in the
            // `val` param slot, the masked count runs from `__scr`.
            "__shl_u8" => {
                self.emit_shift_body(1, BinOp::Shl, scr);
            }
            "__lshr_u8" => {
                self.emit_shift_body(1, BinOp::LShr, scr);
            }
            "__ashr_i8" => {
                self.emit_shift_body(1, BinOp::AShr, scr);
            }
            "__shl_u16" => {
                self.emit_shift_body(2, BinOp::Shl, scr);
            }
            "__lshr_u16" => {
                self.emit_shift_body(2, BinOp::LShr, scr);
            }
            "__ashr_i16" => {
                self.emit_shift_body(2, BinOp::AShr, scr);
            }
            "__shl_u32" => {
                self.emit_shift_body(4, BinOp::Shl, scr);
            }
            "__lshr_u32" => {
                self.emit_shift_body(4, BinOp::LShr, scr);
            }
            "__ashr_i32" => {
                self.emit_shift_body(4, BinOp::AShr, scr);
            }
            other => panic!("isel: runtime routine @{other} has no baseline recipe (float routines are P7 scope)"),
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

/// The recipe selecting a runtime routine's body. Baseline has no
/// interrupt context, so no `_isr` copies exist; the strip stays for
/// shape-parity with classic `isel` (a duplicated user function strips
/// to a non-routine and takes the ordinary path).
fn routine_recipe(name: &str) -> Option<&str> {
    let base = name.strip_suffix("_isr").unwrap_or(name);
    ir::is_runtime_routine(name).then_some(base)
}

/// Emit one function's body into `g.out`.
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    g.cur_loc = None;
    if ir::is_runtime_routine(&f.name) {
        g.emit_routine();
        return;
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
                            g.emit(format!("    MOVF {}, W", g.fop(ca)));
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

/// Reader entry cost (MOVWF/MOVLW/ADDWF/MOVWF PCL) before a table's
/// RETLWs. A table of N bytes occupies 4 + N words in its page low
/// half, so N tops out at 252: bigger tables fit no 256-word window
/// (D-5) and are rejected, never chunked.
const READER_WORDS: usize = 4;
const TABLE_MAX: usize = 252;

/// Flash-resident consts: `is_const` globals with no RAM address.
/// RAM-copied consts (in `addrs`) initialize from `__start` instead.
fn flash_consts<'m>(m: &'m Module, addrs: &HashMap<String, u16>) -> Vec<&'m ir::Global> {
    m.globals
        .iter()
        .filter(|g| g.is_const && !addrs.contains_key(&g.name))
        .collect()
}

/// Word cost of `lines` under org/label semantics: labels, equ, org,
/// list/radix/end, blanks and comments cost nothing; every other line
/// is one baseline word. Placement and the page-fit audit share it, so
/// a miscount fails the audit rather than silently shifting tables.
fn count_words(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|l| {
            let t = l.split(';').next().unwrap_or("").trim();
            if t.is_empty() || t.ends_with(':') || t.contains(" equ ") {
                return false;
            }
            !(t.starts_with("org ")
                || t.starts_with("list ")
                || t.starts_with("radix ")
                || t == "end")
        })
        .count()
}

/// Patch recorded PA0 placeholder bits in `lines` after layout: each
/// fixup names whose page its bit must select. The patched line must
/// still be the placeholder (bit swaps never move text), so a drifted
/// index fails loudly instead of flipping an arbitrary instruction.
fn patch_pa0(
    lines: &mut [String],
    fixups: &[(usize, Pa0Page)],
    func_page: &HashMap<String, u8>,
    table_page: &HashMap<String, u8>,
) {
    for (idx, page) in fixups {
        let p = match page {
            Pa0Page::Func(f) if f == "__start" => 0,
            Pa0Page::Func(f) => *func_page
                .get(f)
                .unwrap_or_else(|| panic!("isel: PA0 fixup for unplaced function @{f}")),
            Pa0Page::Table(t) => *table_page
                .get(t)
                .unwrap_or_else(|| panic!("isel: PA0 fixup for unplaced table @{t}")),
        };
        let line = &mut lines[*idx];
        assert!(
            line.trim() == "BCF STATUS, 5",
            "isel: PA0 fixup landed on non-placeholder {line:?}"
        );
        if p == 1 {
            *line = "    BSF STATUS, 5".to_string();
        }
    }
}

/// One sequential scan: org tracking, label addresses, CALLs with the
/// PA0 selecting them, RETLW runs, and every word's text (to audit
/// reader immediates against placement).
struct AsmScan {
    labels: HashMap<String, usize>,
    calls: Vec<(usize, String, u8)>,
    gotos: Vec<(usize, String, u8)>,
    table_retlws: HashMap<String, usize>,
    words: HashMap<usize, String>,
    end_org: usize,
}

/// Scan `asm` for the page-fit audit. `tables` maps flash-const names
/// to byte lengths so RETLW runs count against the right table.
fn scan_asm(asm: &str, tables: &HashMap<String, usize>) -> AsmScan {
    let mut labels = HashMap::new();
    let mut calls = Vec::new();
    let mut gotos = Vec::new();
    let mut table_retlws: HashMap<String, usize> = HashMap::new();
    let mut words: HashMap<usize, String> = HashMap::new();
    let mut org = 0usize;
    let mut pa0 = 0u8;
    let mut cur_table: Option<String> = None;
    for raw in asm.lines() {
        let t = raw.split(';').next().unwrap_or("").trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16)
                .unwrap_or_else(|_| panic!("isel: malformed org {t:?}"));
            cur_table = None;
            continue;
        }
        if t == "end" {
            break;
        }
        if t.ends_with(':') && !t.contains(' ') {
            let name = t.trim_end_matches(':').to_string();
            labels.insert(name.clone(), org);
            cur_table = if tables.contains_key(&name) {
                Some(name)
            } else {
                None
            };
            continue;
        }
        if t.contains(" equ ") || t.starts_with("list ") || t.starts_with("radix ") {
            continue;
        }
        let nospace: String = t.chars().filter(|c| *c != ' ').collect();
        if nospace == "BSFSTATUS,5" {
            pa0 = 1;
        } else if nospace == "BCFSTATUS,5" {
            pa0 = 0;
        }
        if t.starts_with("CALL ") {
            let target = t
                .split_whitespace()
                .nth(1)
                .unwrap_or_else(|| panic!("isel: malformed CALL {t:?}"));
            calls.push((org, target.to_string(), pa0));
        } else if t.starts_with("GOTO ") {
            let target = t
                .split_whitespace()
                .nth(1)
                .unwrap_or_else(|| panic!("isel: malformed GOTO {t:?}"));
            gotos.push((org, target.to_string(), pa0));
        } else if t.starts_with("RETLW") {
            if let Some(name) = &cur_table {
                *table_retlws.entry(name.clone()).or_insert(0) += 1;
            }
        } else {
            cur_table = None;
        }
        words.insert(org, t.to_string());
        org += 1;
    }
    AsmScan {
        labels,
        calls,
        gotos,
        table_retlws,
        words,
        end_org: org,
    }
}
/// Page audit (D-5): entries in a page low half, each `__read_` entry +
/// table inside one page's low half with the full RETLW run, each CALL
/// targeting a low half of the PA0-selected page, and each GOTO
/// targeting the PA0-selected page. Any crossing panics: a table or
/// branch past its ceiling is rejected here, never silently miscompiled.
pub fn verify_page_fit(m: &Module, asm: &str, addrs: &HashMap<String, u16>) {
    let tables: HashMap<String, usize> = flash_consts(m, addrs)
        .iter()
        .map(|g| (g.name.clone(), g.bytes.len()))
        .collect();
    let scan = scan_asm(asm, &tables);
    // Function entries are CALL targets: CALL cannot reach a high
    // half, and P4 manages no page-1 code, so entries stay in the
    // page-0 low half. Local labels ride GOTOs (full-page reach) and
    // fall-through, so page 0 suffices for them.
    let mut entries: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    entries.insert("__start".to_string());
    for (name, addr) in &scan.labels {
        if let Some(reader) = name.strip_prefix("__read_") {
            let n = tables
                .get(reader)
                .unwrap_or_else(|| panic!("isel: reader {name} has no flash const table"));
            assert!(
                    (addr & 0x1FF) + READER_WORDS + n <= 0x100,
                    "isel: table @{reader} crosses the 256-word ceiling (entry at {addr:#x}, {n} bytes, D-5)"
                );
            let base = scan
                .labels
                .get(reader)
                .unwrap_or_else(|| panic!("isel: table @{reader} has no base label"));
            assert!(
                *base == addr + READER_WORDS,
                "isel: table @{reader} base at {base:#x}, expected {:#x}",
                addr + READER_WORDS
            );
            assert_eq!(
                scan.table_retlws.get(reader).copied().unwrap_or(0),
                *n,
                "isel: table @{reader} emits {} RETLWs, expected {n}",
                scan.table_retlws.get(reader).copied().unwrap_or(0)
            );
            // The entry shape is fixed (MOVWF/MOVLW/ADDWF/MOVWF PCL):
            // audit the base immediate against placement, closing the
            // loop between the cursor math and the emitted text.
            let word_at = |off: usize| {
                scan.words
                    .get(&(addr + off))
                    .unwrap_or_else(|| {
                        panic!(
                            "isel: table @{reader} entry word {off} missing (at {:#x})",
                            addr + off
                        )
                    })
                    .clone()
            };
            assert!(
                word_at(0).starts_with("MOVWF"),
                "isel: table @{reader} entry is not MOVWF: {}",
                word_at(0)
            );
            assert_eq!(
                word_at(1),
                format!("MOVLW 0x{:02X}", ((addr + READER_WORDS) & 0xFF) as u8),
                "isel: table @{reader} base immediate disagrees with placement"
            );
            assert!(
                word_at(2).starts_with("ADDWF"),
                "isel: table @{reader} entry +2 is not ADDWF: {}",
                word_at(2)
            );
            assert_eq!(
                word_at(3),
                "MOVWF PCL",
                "isel: table @{reader} entry +3 is not MOVWF PCL: {}",
                word_at(3)
            );
        } else if tables.contains_key(name.as_str()) {
            continue;
        } else if entries.contains(name.as_str()) {
            assert!(
                (*addr & 0x1FF) < 0x100,
                "isel: function entry {name} at {addr:#x} escapes every page low half (D-5)"
            );
        } else {
            assert!(
                *addr < 0x400,
                "isel: code label {name} at {addr:#x} escapes flash (D-5)"
            );
        }
    }
    for name in tables.keys() {
        assert!(
            scan.labels.contains_key(&format!("__read_{name}")),
            "isel: flash const @{name} has no reader entry"
        );
    }
    for (addr, target, pa0) in &scan.calls {
        let t = scan
            .labels
            .get(target)
            .unwrap_or_else(|| panic!("isel: CALL {target} has no label (at {addr:#x})"));
        assert!(
            *t < 0x400 && (*t & 0x1FF) < 0x100,
            "isel: CALL {target} at {t:#x} escapes every page low half (D-5)"
        );
        assert_eq!(
            (*t >> 9) as u8,
            *pa0,
            "isel: CALL {target} targets page {} but PA0 selects {pa0} (at {addr:#x})",
            *t >> 9
        );
    }
    // GOTO targets must sit in the containing fragment's page: a GOTO
    // loads PC<8:0> from its literal with PC<9> from PA0, and PA0
    // inside a fragment is the fragment's own page (set by every
    // caller before CALL). Positional PA0 tracking cannot see this
    // (callee bodies live in other fragments), so attribute by
    // address: the nearest entry label at or below the GOTO opens its
    // fragment, whose page is the entry address's page. Anything
    // before the first entry is head code in page 0.
    let mut entries: Vec<(usize, u8)> = scan
        .labels
        .iter()
        .filter(|(name, _)| {
            name.as_str() == "__start"
                || name.starts_with("__read_")
                || m.funcs.iter().any(|f| f.name.as_str() == name.as_str())
                || tables.contains_key(name.as_str())
        })
        .map(|(_, addr)| (*addr, (*addr >> 9) as u8))
        .collect();
    entries.sort();
    for (addr, target, _) in &scan.gotos {
        let t = scan
            .labels
            .get(target)
            .unwrap_or_else(|| panic!("isel: GOTO {target} has no label (at {addr:#x})"));
        let page = entries
            .iter()
            .rev()
            .find(|(e, _)| *e <= *addr)
            .map(|(_, p)| *p)
            .unwrap_or(0);
        assert_eq!(
            (*t >> 9) as u8,
            page,
            "isel: GOTO {target} targets page {} but its fragment sits in page {page} (at {addr:#x})",
            *t >> 9
        );
    }
    assert!(
        scan.end_org <= 0x400,
        "isel: program ({} words) overflows the 509's 1024-word flash",
        scan.end_org
    );
}

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
    // bytes): scratch at 0x07, retval at 0x08-0x0B, scratch2 at 0x0C,
    // store_tmp at 0x0D, leaving 0x0E-0x0F free.
    let (common_lo, common_hi) = device
        .common_ram
        .expect("isel's fixed scratch/retval layout needs a common-RAM region");
    let scratch: u16 = common_lo;
    let retval_lo: u16 = common_lo + 1;
    // scratch2 (the ADDLW-replacement temp) sits right after the retval
    // region: common RAM 0x07-0x0F = scratch(0x07) + retval(0x08-0x0B) +
    // scratch2(0x0C) + store_tmp(0x0D), leaving 0x0E-0x0F free.
    let scratch2: u16 = common_lo + 5;
    let store_tmp: u16 = common_lo + 6;
    assert!(
        retval_lo + 4 <= common_hi + 1,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must fit in common RAM",
        retval_lo + 3
    );
    assert!(
        store_tmp <= common_hi,
        "isel: store_tmp 0x{store_tmp:02X} must fit in common RAM (0x{common_lo:02X}-0x{common_hi:02X})"
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
    // before main runs. Each write is a direct access, so it needs the
    // D-2 bank reassertion and the 5-bit within-bank mask (the 509's FSR
    // power-on reset has bit 5 = 1, so a bank-0 const needs BCF FSR,5).
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
                let addr = base + i as u16;
                match device.bank_of(addr) {
                    Some(0) => init.push("    BCF FSR, 5".to_string()),
                    Some(1) => init.push("    BSF FSR, 5".to_string()),
                    _ => {}
                }
                init.push(format!("    MOVWF 0x{:02X}", addr & 0x1F));
            }
        }
    }
    // The head's managed CALL (`CALL main`) records fixups on `out`
    // directly: `__start` always lives in page 0.
    let mut head_fixes: Vec<(usize, Pa0Page)> = Vec::new();
    // `__start` opens with PA0 = 0 (page 0 is the caller page for every
    // const read) and the RAM-const init: both must execute, so both sit
    // after the label `goto __start` lands on, before `CALL main`.
    let mut start_full: Vec<String> = vec!["__start:".to_string(), "    BCF STATUS, 5".to_string()];
    start_full.extend(init);
    // `CALL main` managed like every call: set for main's page, restore
    // to page 0 (`__start` context). Fixups index into `out`, so record
    // against its length once extended below.
    start_full.push("    BCF STATUS, 5".to_string());
    start_full.push("    CALL main".to_string());
    start_full.push("    BCF STATUS, 5".to_string());
    start_full.extend(vec!["    SLEEP".to_string(), "".to_string()]);
    let start_len = start_full.len();
    let head_base = out.len();
    head_fixes.push((head_base + start_len - 5, Pa0Page::Func("main".to_string())));
    head_fixes.push((
        head_base + start_len - 3,
        Pa0Page::Func("__start".to_string()),
    ));
    out.extend(start_full);
    locs.extend(std::iter::repeat(None).take(start_len));
    // Pointers resolve eagerly: every GEP chain folds to `(base, k, terms)`.
    let resolved = resolve_pointers(m);
    // Emit each function into its own buffer (PA0 fixups recorded per
    // buffer); layout assigns pages after measuring, then patches the
    // bits in place. Call sequences are uniform size, so one pass
    // measures exactly.
    let mut tmp = 0u32;
    let mut frags: Vec<(
        String,
        Vec<String>,
        Vec<Option<SrcLoc>>,
        Vec<(usize, Pa0Page)>,
    )> = Vec::new();
    for f in &m.funcs {
        let mut fixes = Vec::new();
        let mut g = Gen {
            m,
            addrs,
            device,
            resolved: &resolved,
            scratch,
            scratch2,
            store_tmp,
            retval_lo,
            cur_func: &f.name,
            fixups: &mut fixes,
            tmp: &mut tmp,
            w_holds: None,
            cur_loc: None,
            out: Vec::new(),
            locs: Vec::new(),
        };
        emit_func_body(&mut g, f);
        frags.push((f.name.clone(), g.out, g.locs, fixes));
    }
    let head_words = count_words(&out);
    // Layout: each function wholly in one page with its entry in a low
    // half (CALL cannot reach a high half); the cursor jumps pages with
    // `org`. Emission order is kept, so fall-through never changes
    // neighbors.
    let mut cursor = head_words;
    let mut func_page: HashMap<String, u8> = HashMap::new();
    let mut func_entry: HashMap<String, usize> = HashMap::new();
    for (name, code, _, _) in &frags {
        let size = count_words(code);
        assert!(
            size <= 0x200,
            "isel: function @{name} ({size} words) fits no 512-word page (D-5)"
        );
        loop {
            assert!(
                cursor < 0x400,
                "isel: function @{name} overflows the 1024-word flash (D-5)"
            );
            let in_low = (cursor & 0x1FF) < 0x100;
            // The whole body must fit the current page: spanning a page
            // boundary strands forward branch targets in the PA0-other
            // page (GOTOs cannot cross pages).
            let fits_page = cursor + size <= (cursor & !0x1FF) + 0x200;
            if in_low && fits_page {
                break;
            }
            cursor = (cursor & !0x1FF) + 0x200;
        }
        func_page.insert(name.clone(), (cursor >> 9) as u8);
        func_entry.insert(name.clone(), cursor);
        cursor += size;
    }
    // Const tables pack the page low halves around the functions: occ0
    // and occ1 track each low half's occupied end (head plus function
    // extents clipped to the half). First-fit page 0, spill page 1.
    let consts = flash_consts(m, addrs);
    let mut occ = [head_words.min(0x100), 0x200];
    for (name, code, _, _) in &frags {
        let (e, s) = (func_entry[name.as_str()], count_words(code));
        if e < 0x100 {
            occ[0] = occ[0].max((e + s).min(0x100));
        } else {
            occ[1] = occ[1].max((e + s).min(0x300));
        }
    }
    let mut placed: Vec<(String, usize, u8)> = Vec::new();
    for g in consts {
        let n = g.bytes.len();
        assert!(
            n <= TABLE_MAX,
            "isel: const @{} too large ({n} bytes; no page low half fits 4 reader words + {n} RETLWs, D-5)",
            g.name
        );
        let need = READER_WORDS + n;
        if occ[0] + need <= 0x100 {
            placed.push((g.name.clone(), occ[0], 0));
            occ[0] += need;
        } else if occ[1] + need <= 0x300 {
            placed.push((g.name.clone(), occ[1], 1));
            occ[1] += need;
        } else {
            panic!(
                "isel: const @{} crosses the 256-word ceiling (page-0 low half full at {:#x}, page-1 low half full at {:#x}, D-5)",
                g.name, occ[0], occ[1]
            );
        }
    }
    // Emit page-0 tables before page-1 ones: the join walks this vector
    // behind orgs, and grouped pages keep one transition.
    placed.sort_by_key(|t| t.2);
    let table_page: HashMap<String, u8> = placed
        .iter()
        .map(|(name, _, page)| (name.clone(), *page))
        .collect();
    // Patch every recorded PA0 bit now that pages are final. Bits patch
    // in place (BCF/BSF swaps), so measured sizes hold.
    patch_pa0(&mut out, &head_fixes, &func_page, &table_page);
    let mut running = head_words;
    for (name, code, flocs, fixes) in &mut frags {
        let entry = func_entry[name.as_str()];
        if entry != running {
            out.push(format!("    org 0x{entry:03X}"));
            locs.push(None);
        }
        patch_pa0(code, fixes, &func_page, &table_page);
        running = entry + count_words(code);
        out.extend(code.drain(..));
        locs.extend(flocs.drain(..));
    }
    // The const section: readers + RETLW tables with numeric bases (isel
    // placed them); the page-fit audit re-derives every address.
    for (name, entry, _) in &placed {
        if *entry != running {
            out.push(format!("    org 0x{entry:03X}"));
            locs.push(None);
        }
        let g = m
            .globals
            .iter()
            .find(|g| &g.name == name)
            .unwrap_or_else(|| panic!("isel: placed table @{name} is not a global"));
        let base = entry + READER_WORDS;
        out.push(format!("__read_{name}:"));
        locs.push(None);
        out.push("    MOVWF 0x07".to_string());
        locs.push(None);
        out.push(format!("    MOVLW 0x{:02X}", (base & 0xFF) as u8));
        locs.push(None);
        out.push("    ADDWF 0x07, W".to_string());
        locs.push(None);
        out.push("    MOVWF PCL".to_string());
        locs.push(None);
        out.push(format!("{name}:"));
        locs.push(None);
        for (i, b) in g.bytes.iter().enumerate() {
            // A function-address field materializes the link-time label
            // literal, mirroring classic isel: byte 0 = LOW(fn), byte 1 =
            // HIGH(fn), resolved by the assembler's symbol table.
            if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                out.push(format!("    RETLW {lit}({f})"));
            } else {
                out.push(format!("    RETLW 0x{b:02X}"));
            }
            locs.push(None);
        }
        out.push("".to_string());
        locs.push(None);
        running = entry + READER_WORDS + g.bytes.len();
    }
    out.push("    end".to_string());
    locs.push(None);
    (out.join("\n"), locs)
}

/// `parse_map` lives in `iselcore` now: it is a plain text-format parser
/// over `alloc`'s output with nothing PIC14-specific about it. Re-exported
/// here so this crate's binary keeps working unchanged.
pub use iselcore::parse_map;
