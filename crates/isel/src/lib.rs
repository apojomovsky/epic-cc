//! `isel` — instruction selection for the integer spine.
//!
//! Scalar surface (milestones 2-6): lowers `load`/`store`, the full binop
//! set `add`/`sub`/`and`/`or`/`xor` (i8 and i16), `zext`/`sext`/`trunc`,
//! all ten `icmp` predicates (eq/ne/ult/ule/ugt/uge/slt/sle/sgt/sge),
//! `call` (arg copies into the callee's param slots + retval copy), `ret`
//! with and without a value, and eliminates `phi` by copying each
//! predecessor's incoming value into the phi destination at the end of the
//! predecessor block, before its terminator. Any other instruction or binop
//! panics.
//!
//! Phase-3 pointers/const (M5-M7): `gep` defines a *virtual* pointer
//! (emits nothing), lowered at each `load`/`store`/`memcpy` use. Every GEP
//! chain is resolved eagerly to a `(Base, k, terms)` triple: `Base::Global`
//! (RAM or const-flash), `Base::Slot(name, indirect)` — a byval param copy,
//! an alloca, or an sret slot holding a target address (indirect). A
//! constant offset (no terms) reads/writes the plain file register; dynamic
//! terms set `FSR` to `base + k + Σ s×%r` (single scale-1 term keeps the M5
//! `MOVF %r,W; ADDLW base+k; MOVWF FSR` fast path; general sums accumulate
//! in the fixed scratch byte); an indirect base takes the target address
//! from the slot's contents. Pointers into const (flash) globals load via
//! `CALL __read_<name>` — a RETLW table emitted after the functions — and a
//! store through a const base panics (ROM is not writable). `memcpy`
//! lowers to a byte loop of the same pointer machinery; `alloca` is virtual
//! like `gep` (the slot is sized by alloc). Every FSR base must sit in the
//! low 256 bytes (bank 0 — IRP multi-bank FSR is a later milestone),
//! asserted loudly at emission.
//!
//! Every value's address comes from the caller-supplied address map: globals
//! by name, locals by `{func}::{name}` (IR value names without `%`). isel
//! performs no slot allocation; it trusts the map (from `alloc`'s overlay
//! layout) and panics loudly if a value is missing from it.

use ir::{BinOp, Gep, GepBase, Inst, Module, Ty, Val};
use std::collections::HashMap;

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// A resolved GEP base: a named global (`@g`) or a local slot. A slot may
/// be *indirect* (an sret param): it holds the target address, so FSR is
/// taken from its contents rather than the slot itself being the base.
#[derive(Clone, Debug)]
enum Base {
    Global(String),
    Slot(String, bool),
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
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)` — GEP chains fully collapsed (base
    /// `Reg` replaced by the base's own entry), plus the seeded pointer
    /// bases (byval/sret params and allocas). `gep`/`alloca` themselves
    /// emit nothing; each `load`/`store`/`memcpy` through a pointer reg
    /// lowers the pointer at its use.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    scratch: u16,
    retval_lo: u16,
    cur_func: &'m str,
    /// Module-scoped fresh-label counter, shared across every function so the
    /// emitted `tmp{n}:` labels stay unique in the single `.asm` output.
    tmp: &'m mut u32,
    out: Vec<String>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    /// Resolve `{func}::{name}` to its base byte address (lo for multi-byte).
    /// Every address comes from the caller-supplied map; a missing value
    /// panics loudly rather than being allocated internally.
    fn slot_addr(&self, func: &str, name: &str) -> u16 {
        *self
            .addrs
            .get(&ssa_key(func, name))
            .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}"))
    }

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> u16 {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
            Val::Global(g) => *self
                .addrs
                .get(g)
                .unwrap_or_else(|| panic!("isel: no address for @{g}")),
            Val::Const(k) => {
                assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
                *k as u16
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

    /// Whether `name` is a const (flash) global — read via RETLW tables.
    fn global_is_const(&self, name: &str) -> bool {
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .is_const
    }

    /// The resolved `(base, k, terms)` for a pointer reg `%r` — a GEP dst,
    /// or a seeded byval/sret param / alloca. Anything else is a missing
    /// pointer and panics loudly.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
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
                match base {
                    Base::Global(name) => {
                        assert!(
                            !self.global_is_const(&name),
                            "isel: store to const (flash) global @{name}"
                        );
                        if terms.is_empty() {
                            // Constant offset only: the address is statically
                            // known — a plain file-register access, no FSR.
                            Addr::Direct(
                                self.global_addr(&name) + u16::from(k) + u16::from(byte_off),
                            )
                        } else {
                            self.emit_fsr_to(self.global_addr(&name), k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, &sname);
                        if !indirect && terms.is_empty() {
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
                        } else if !indirect {
                            self.emit_fsr_to(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else {
                            self.emit_fsr_indirect(sa, k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// `W = RAM[ptr + byte_off]` — one byte of a pointer load or a memcpy
    /// source. Direct bases read the plain file register; dynamic bases set
    /// FSR first and read INDF; a const (flash) base reads via
    /// `CALL __read_<name>` (the RETLW table leaves the byte in W).
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), k, terms) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        // RETLW table read: W = index = k + Σ s×%reg + off.
                        self.emit_ptr_index_w(k, &terms, byte_off);
                        self.emit(format!("    CALL __read_{name}"));
                        return;
                    }
                }
            }
            Val::Global(g) => {
                if self.global_is_const(g) {
                    // A const global used directly as a pointer (memcpy src):
                    // W = byte index, CALL the RETLW table reader.
                    self.emit(format!("    MOVLW 0x{byte_off:02X}"));
                    self.emit(format!("    CALL __read_{g}"));
                    return;
                }
            }
            Val::Const(_) => panic!("isel: load through a constant pointer"),
        }
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => self.emit(format!("    MOVF 0x{a:02X}, W")),
            Addr::Indirect => self.emit("    MOVF INDF, W".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = W` — the store side of a byte access (memcpy
    /// destinations; `emit_ptr_store_byte` composes a val load before it).
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => self.emit(format!("    MOVWF 0x{a:02X}")),
            Addr::Indirect => self.emit("    MOVWF INDF".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        // The address setup comes first — its FSR/scratch computation
        // clobbers W, so the value is loaded only after FSR is final.
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_load_byte(val, byte_off);
                self.emit(format!("    MOVWF 0x{a:02X}"));
            }
            Addr::Indirect => {
                self.emit_load_byte(val, byte_off);
                self.emit("    MOVWF INDF".to_string());
            }
        }
    }

    /// `FSR = base_addr + k + byte_off + Σ scale×%reg`. A single scale-1
    /// term keeps the M5 fast shape (`MOVF %r,W; ADDLW base+k; MOVWF FSR`);
    /// general sums accumulate in the fixed scratch byte first. The static
    /// FSR base (before the runtime terms) must sit in bank 0 (≤ 0xFF) —
    /// IRP multi-bank FSR is a later milestone, so anything past it fails
    /// loudly rather than emitting an unrepresentable ADDLW literal.
    fn emit_fsr_to(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let base_k = base_addr + u16::from(k) + u16::from(byte_off);
        assert!(
            base_k <= 0xFF,
            "isel: FSR base 0x{base_k:02X} (base 0x{base_addr:02X} + k {k} + off {byte_off}) out of bank-0 range (IRP follow-up)"
        );
        match terms {
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDLW 0x{base_k:02X}"));
                self.emit("    MOVWF FSR".to_string());
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit(format!("    ADDLW 0x{base_k:02X}"));
                self.emit("    MOVWF FSR".to_string());
            }
        }
    }

    /// Indirect (sret) FSR setup: `FSR = [slot] + k + byte_off + Σ terms`.
    /// The slot holds the target address (the caller's sret ABI asserts it
    /// ≤ 0xFF when it is stored); the static k + off must fit the ADDLW
    /// literal.
    fn emit_fsr_indirect(&mut self, slot_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: indirect offset k {k} + off {byte_off} out of byte range"
        );
        if terms.is_empty() {
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit(format!("    ADDLW 0x{kk:02X}"));
            self.emit("    MOVWF FSR".to_string());
        } else {
            self.emit_accum_terms(terms);
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
            self.emit(format!("    ADDLW 0x{kk:02X}"));
            self.emit("    MOVWF FSR".to_string());
        }
    }

    /// `scratch = Σ scale×%reg`: W = 0, then per term
    /// `MOVF %r,W; ADDWF scratch,W; MOVWF scratch` repeated `scale` times.
    /// ADDWF f,W computes W = f + W, so W holds %r only until the first
    /// ADDWF — it MUST be reloaded before each repetition or a scaled term
    /// accumulates 2×scratch + %r (silent wrong-address miscompile).
    fn emit_accum_terms(&mut self, terms: &[(u8, String)]) {
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        for (scale, r) in terms {
            let a = self.val_addr(&Val::Reg(r.clone()));
            for _ in 0..*scale {
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            }
        }
    }

    /// `W = k + byte_off + Σ scale×%reg` — the byte index into a const
    /// (flash) table before `CALL __read_<name>`. A single scale-1 term
    /// keeps the M5 `MOVF %r,W` shape (ADDLW only when k + off is nonzero);
    /// general sums accumulate in scratch.
    fn emit_ptr_index_w(&mut self, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(kk <= 0xFF, "isel: const index k {k} + off {byte_off} out of byte range");
        match terms {
            [] => self.emit(format!("    MOVLW 0x{kk:02X}")),
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                if kk != 0 {
                    self.emit(format!("    ADDLW 0x{kk:02X}"));
                }
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit(format!("    ADDLW 0x{kk:02X}"));
            }
        }
    }

    /// W = byte `idx` of `val`.
    fn emit_load_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    MOVF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
                self.emit(format!("    MOVF 0x{:02X}, W", a + u16::from(idx)));
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
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
        }
    }

    /// Copy `val` (width `ty`) into the slot starting at `dst`.
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u16) {
        for i in 0..ty.bytes() {
            self.emit_load_byte(val, i);
            self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
        }
    }

    /// Set the Z flag to (a == b) without disturbing other flags. For i16,
    /// the XORs of both bytes are accumulated in the fixed `scratch` byte,
    /// leaving Z set exactly when every byte pair was equal.
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
    /// `signed` and `i` is the high (sign) byte. Complementing the sign bit
    /// maps signed order onto unsigned order — signed(a >= b) ==
    /// unsigned((a ^ 0x80) >= (b ^ 0x80)) — so one flag recipe serves both.
    fn emit_load_cmp_byte(&mut self, v: &Val, i: u8, signed: bool, high: u8) {
        match v {
            Val::Const(k) => {
                let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                let b = if signed && i == high { byte ^ 0x80 } else { byte };
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            _ => {
                let addr = self.val_addr(v) + u16::from(i);
                if signed && i == high {
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit(format!("    XORWF 0x{addr:02X}, W"));
                } else {
                    self.emit(format!("    MOVF 0x{addr:02X}, W"));
                }
            }
        }
    }

    /// Set C = (a >= b) — unsigned or signed (sign-bit complement). For i8
    /// the SUBWF/SUBLW also leaves Z = (a == b); for i16 the borrow chain's
    /// final Z is only a byte-level flag, so predicates needing equality
    /// append `emit_cmp_eq` (which preserves C). A const RHS becomes the
    /// MOVLW/SUBWF subtrahend; a const LHS uses SUBLW (k - W) since a const
    /// can never be read as a file register.
    fn emit_cmp_c(&mut self, a: &Val, b: &Val, ty: Ty, signed: bool) {
        let n = ty.bytes();
        let high = n - 1;
        match (a, b) {
            (Val::Const(_), Val::Const(_)) => panic!("isel: constant folding not implemented"),
            (Val::Const(k), _) => {
                // SUBLW chain: W holds the b byte (+ borrow); SUBLW subtracts
                // it from the const byte, so C = (a >= b).
                self.emit_load_cmp_byte(b, 0, signed, high);
                let k0 = (k & 0xFF) as u8;
                // The low byte is the sign byte for i8: fold the complement
                // in when signed.
                let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
                self.emit(format!("    SUBLW 0x{k0:02X}"));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit("    ADDLW 0x01".to_string());
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    let kb = if signed && i == high { kb ^ 0x80 } else { kb };
                    self.emit(format!("    SUBLW 0x{kb:02X}"));
                }
            }
            _ => {
                let aa = self.val_addr(a);
                let use_scratch = signed; // signed file-LHS: SUBWF's file operand must be a ^ 0x80
                if use_scratch {
                    // Pre-store the complemented sign byte; MOVLW/XORWF/MOVWF
                    // do not touch C, and the low-byte SUBWF below sets it.
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit(format!("    XORWF 0x{:02X}, W", aa + high as u16));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                }
                self.emit_load_cmp_byte(b, 0, signed, high);
                self.emit(format!(
                    "    SUBWF 0x{:02X}, W",
                    if use_scratch && n == 1 { self.scratch } else { aa }
                ));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit("    ADDLW 0x01".to_string());
                    let f = if i == high && use_scratch {
                        self.scratch
                    } else {
                        aa + i as u16
                    };
                    self.emit(format!("    SUBWF 0x{f:02X}, W"));
                }
            }
        }
    }

    /// Materialize a flag predicate into `dst` as an i1. `cond` reads the C
    /// and/or Z flags left by the immediately preceding compare/accumulation
    /// (only MOVF/MOVLW/MOVWF/XORWF/XORLW/IORWF between, which never touch
    /// C). `Z` is the eq materialization; `!Z` (ne) inverts it; `C`/`!C`
    /// materialize uge/ult (and sge/slt); `C&&!Z`/`!C||Z` materialize
    /// ugt/ule (and sgt/sle).
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
            // Second condition: C&&!Z clears the 1 when Z is set (equal);
            // !C||Z sets it when Z is set. BTFSC STATUS,2 skips the
            // adjustment when Z is clear.
            self.emit("    BTFSC STATUS, 2 ; Z".to_string());
            self.emit(format!("    {adj2}"));
        }
        self.emit(format!("    MOVWF 0x{dst:02X}"));
    }

    /// Branch on `cond`: Z = (cond == 0); if Z is set (cond == 0) go to `f`,
    /// otherwise (cond != 0) go to `t`. Mirrors spike emit_cond_branch.
    fn emit_cond_branch(&mut self, cond: &Val, t: &str, f: &str) {
        match cond {
            Val::Reg(r) => {
                let ca = self.val_addr(&Val::Reg(r.clone()));
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

    /// `d = cond ? a : b` via an if/else jump over two copies. Mirrors spike
    /// emit_select.
    fn emit_select(&mut self, dst: &str, cond: &Val, ty: Ty, a: &Val, b: &Val) {
        let da = self.slot_addr(self.cur_func, dst);
        match cond {
            Val::Const(k) => {
                let v = if *k != 0 { a } else { b };
                self.emit_move_val_to_slot(v, ty, da);
                return;
            }
            Val::Global(_) => panic!("isel: select condition is a global"),
            Val::Reg(_) => {}
        }
        let l_else = self.fresh_label();
        let l_end = self.fresh_label();
        let ca = match cond {
            Val::Reg(r) => self.val_addr(&Val::Reg(r.clone())),
            _ => unreachable!(),
        };
        self.emit(format!("    MOVF 0x{ca:02X}, W"));
        self.emit("    BTFSC STATUS, 2 ; Z".to_string());
        self.emit(format!("    GOTO {l_else}"));
        self.emit_move_val_to_slot(a, ty, da);
        self.emit(format!("    GOTO {l_end}"));
        self.emit(format!("{l_else}:"));
        self.emit_move_val_to_slot(b, ty, da);
        self.emit(format!("{l_end}:"));
    }

    /// A fresh local label for intra-block jumps (select branches). The
    /// counter lives at module scope so labels are unique across functions.
    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.tmp);
        *self.tmp += 1;
        s
    }

    /// `d = a + b` for i16 (either operand may be a register; at most one a
    /// const). Low byte adds, then the high byte adds with the carry from the
    /// low byte folded in via BTFSC/ADDLW.
    fn emit_add16(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add16 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg));
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVF 0x{ra:02X}, W"));
                self.emit(format!("    ADDLW 0x{lo:02X}"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", ra + 1));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    ADDLW 0x{hi:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(_) => panic!("isel: add16 with a global operand"),
        }
    }

    /// `d = a OP b` bytewise, for the commutative binops and/or/xor at i8 or
    /// i16. One operand is a register (the file operand), the other a
    /// register or const; a const LHS is swapped to the RHS so the literal
    /// path (`opw`) is used, never reading a const as a file-register
    /// address. `op` is the reg-file mnemonic (`ANDWF`/`IORWF`/`XORWF`),
    /// `opw` the literal mnemonic (`ANDLW`/`IORLW`/`XORLW`).
    fn emit_commutative(&mut self, a: &Val, b: &Val, ty: Ty, dst: u16, op: &str, opw: &str) {
        let n = ty.bytes();
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: {op} needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg));
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
                for i in 0..n {
                    self.emit(format!("    MOVF 0x{:02X}, W", bb + u16::from(i)));
                    self.emit(format!("    {op} 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                }
            }
            Val::Const(k) => {
                for i in 0..n {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVF 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    {opw} 0x{byte:02X}"));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                }
            }
            Val::Global(_) => panic!("isel: {op} with a global operand"),
        }
    }

    /// `d = a - b` for i8: the subtrahend (reg or const) goes in W and SUBWF
    /// subtracts it from the minuend — SUBWF f,W always computes f - W, so
    /// `a` is the file operand. A const LHS is rejected by the caller (sub
    /// is not commutative).
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a);
        match b {
            Val::Const(k) => {
                assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
                self.emit(format!("    MOVLW 0x{:02X}", *k as u8));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b);
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = a - b` for i16: low byte SUBWF, then the high byte with the
    /// borrow from the low byte folded in — if C is clear (borrow), ADDLW 1
    /// bumps the subtrahend byte before the high SUBWF.
    fn emit_sub16(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a);
        match b {
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{lo:02X}"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(_) => panic!("isel: sub16 with a global operand"),
        }
    }

    /// `dst = call func(args)`: copy each arg into the callee's
    /// `{func}::{param}` slots, `CALL func`, then copy the retval slots
    /// (`retval_lo`/`retval_lo+1`) into `dst`. Void calls skip the retval
    /// copy. Mirrors spike emit_call.
    fn emit_call(&mut self, dst: &Option<String>, ty: Option<Ty>, func: &str, args: &[ir::CallArg]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        for (i, arg) in args.iter().enumerate() {
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(func, pname);
            if let Some(size) = arg.byval {
                // byval: copy `size` bytes from the arg's pointer (global /
                // alloca slot / GEP reg) into the callee's param slot — the
                // param slot IS the callee's struct copy (Slot(name, false)),
                // byte by byte through the shared pointer machinery.
                assert_eq!(
                    size,
                    callee.params[i].byval.expect("isel: byval arg for a non-byval param"),
                    "isel: byval size mismatch for arg {i} of @{func}"
                );
                for b in 0..size {
                    self.emit_ptr_load_byte(&arg.val, b);
                    self.emit(format!("    MOVWF 0x{:02X}", pa + u16::from(b)));
                }
            } else if arg.sret {
                // sret: store the target address into the callee's sret param
                // slot (2 bytes). The target is a global or a plain alloca
                // slot; FSR reaches only the low 256 bytes (bank 0 — IRP is a
                // later milestone), so a target past 0xFF fails loudly rather
                // than emitting an address FSR cannot reach.
                let addr = match &arg.val {
                    Val::Global(g) => self.global_addr(g),
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        assert!(
                            k == 0 && terms.is_empty(),
                            "isel: sret target must be a plain global or alloca slot (no offset)"
                        );
                        match base {
                            Base::Global(name) => self.global_addr(&name),
                            Base::Slot(sname, false) => self.slot_addr(self.cur_func, &sname),
                            Base::Slot(_, true) => {
                                panic!("isel: sret target cannot be an indirect (sret) slot")
                            }
                        }
                    }
                    Val::Const(_) => panic!("isel: sret target must be a global or an alloca slot"),
                };
                assert!(
                    addr <= 0xFF,
                    "isel: sret target 0x{addr:02X} out of bank-0 FSR range (IRP follow-up)"
                );
                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa));
                self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
            } else {
                // Scalar args: unchanged.
                let aty = arg.ty.expect("isel: scalar call arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, aty, pa);
            }
        }
        self.emit(format!("    CALL {func}"));
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            let da = self.slot_addr(self.cur_func, d);
            for i in 0..t.bytes() {
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo + u16::from(i)));
                self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
            }
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst);
                if let Some(g) = l.ptr.strip_prefix('@') {
                    let src = self.global_addr(g);
                    for k in 0..l.ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(k)));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                } else {
                    // A GEP-created pointer: const (flash) bases keep the
                    // RETLW path (i8 only); RAM bases go through the shared
                    // byte machinery (direct or FSR/INDF).
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global or %reg", l.ptr)
                    });
                    let ptr = Val::Reg(r.to_string());
                    if let (Base::Global(name), _, _) = self.resolved_for(r) {
                        assert!(
                            !self.global_is_const(&name) || l.ty.bytes() == 1,
                            "isel: multi-byte const load not supported"
                        );
                    }
                    for k in 0..l.ty.bytes() {
                        self.emit_ptr_load_byte(&ptr, k);
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel: only i8/i16 stores supported");
                if let Some(g) = s.ptr.strip_prefix('@') {
                    let dst = self.global_addr(g);
                    self.emit_move_val_to_slot(&s.val, s.ty, dst);
                } else {
                    let r = s.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global or %reg", s.ptr)
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
            Inst::Gep(_) => {} // virtual: lowered at each load/store use
            Inst::Alloca(_) => {} // virtual: the slot is sized by alloc; lowered at each use
            Inst::Memcpy(m) => {
                // Byte loop over the same pointer machinery: src[i] -> dst[i].
                // Each byte re-resolves both pointers (dst itself may be a
                // base+k+i expression), exactly like a per-byte load/store.
                for i in 0..m.len {
                    self.emit_ptr_load_byte(&m.src, i);
                    self.emit_ptr_store_w(&m.dst, i);
                }
            }
            Inst::Bin(b) => {
                assert!(b.ty != Ty::I1, "isel: only i8/i16 binops supported");
                let da = self.slot_addr(self.cur_func, &b.dst);
                match (b.op, b.ty) {
                    (BinOp::Add, Ty::I16) => self.emit_add16(&b.a, &b.b, da),
                    (BinOp::Add, Ty::I8) => {
                        // Normalize commutative add: a const LHS is swapped to
                        // the RHS so the const-adder arm is used, never reading
                        // a const as a file-register address.
                        let (a, b_op) = match (&b.a, &b.b) {
                            (Val::Const(_), Val::Const(_)) => {
                                panic!("isel: constant folding not implemented")
                            }
                            (Val::Const(_), _) => (&b.b, &b.a),
                            _ => (&b.a, &b.b),
                        };
                        match b_op {
                            Val::Const(k) => {
                                assert!(
                                    *k >= 0 && *k <= 255,
                                    "isel: const {k} out of byte range"
                                );
                                let aa = self.val_addr(a);
                                self.emit(format!("    MOVF 0x{aa:02X}, W"));
                                self.emit(format!("    ADDLW 0x{:02X}", *k as u8));
                                self.emit(format!("    MOVWF 0x{da:02X}"));
                            }
                            _ => {
                                let (aa, bb) = (self.val_addr(a), self.val_addr(b_op));
                                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                                self.emit(format!("    ADDWF 0x{aa:02X}, W"));
                                self.emit(format!("    MOVWF 0x{da:02X}"));
                            }
                        }
                    }
                    // Commutative bytewise binops (and/or/xor) share one
                    // emitter for both widths; a const LHS is swapped to the
                    // RHS by emit_commutative.
                    (BinOp::And, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::And, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::Or, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Or, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Xor, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    (BinOp::Xor, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    // sub is NOT commutative: a const LHS (d = k - a) cannot
                    // reuse the reg-const lowering (which computes a - k), so
                    // it must fail loudly rather than silently miscompile.
                    (BinOp::Sub, Ty::I8) => {
                        assert!(
                            !matches!(&b.a, Val::Const(_)),
                            "isel: sub with const LHS not supported (not commutative)"
                        );
                        self.emit_sub8(&b.a, &b.b, da);
                    }
                    (BinOp::Sub, Ty::I16) => {
                        assert!(
                            !matches!(&b.a, Val::Const(_)),
                            "isel: sub with const LHS not supported (not commutative)"
                        );
                        self.emit_sub16(&b.a, &b.b, da);
                    }
                    _ => panic!("isel: unsupported binop for milestone 2"),
                }
            }
            Inst::Zext(z) => {
                assert!(
                    z.from.bytes() < z.to.bytes(),
                    "isel: zext must widen"
                );
                let da = self.slot_addr(self.cur_func, &z.dst);
                for i in 0..z.from.bytes() {
                    self.emit_load_byte(&z.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit(format!("    CLRF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Sext(x) => {
                assert!(
                    x.from.bytes() == 1 && x.to.bytes() == 2,
                    "isel: sext only supports i8 -> i16"
                );
                assert!(
                    !matches!(&x.val, Val::Const(_)),
                    "isel: sext of a constant not supported (constant folding not implemented)"
                );
                let da = self.slot_addr(self.cur_func, &x.dst);
                // Copy the low bytes unchanged.
                for i in 0..x.from.bytes() {
                    self.emit_load_byte(&x.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                // Fill the high bytes with the source's sign bit: test the
                // MSB of the source's high byte, then MOVLW 0xFF (set) or
                // 0x00 (clear) once and store it into every high byte.
                let src_hi = x.from.bytes() - 1;
                let a = self.val_addr(&x.val);
                let l_pos = self.fresh_label();
                let l_fill = self.fresh_label();
                self.emit(format!("    BTFSS 0x{:02X}, 7", a + u16::from(src_hi)));
                self.emit(format!("    GOTO {l_pos}"));
                self.emit("    MOVLW 0xFF".to_string());
                self.emit(format!("    GOTO {l_fill}"));
                self.emit(format!("{l_pos}:"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit(format!("{l_fill}:"));
                for i in x.from.bytes()..x.to.bytes() {
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Trunc(t) => {
                assert!(
                    t.from.bytes() > t.to.bytes(),
                    "isel: trunc must narrow"
                );
                let da = self.slot_addr(self.cur_func, &t.dst);
                for i in 0..t.to.bytes() {
                    self.emit_load_byte(&t.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Icmp(ic) => {
                let da = self.slot_addr(self.cur_func, &ic.dst);
                match ic.pred.as_str() {
                    "eq" => {
                        // XOR-based compare sets Z = (a == b); materialize
                        // the i1. Kept byte-identical.
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("Z", da);
                    }
                    "ne" => {
                        // !Z: the eq compare with the inverted
                        // materialization (BTFSS instead of BTFSC).
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
                        // C = (a >= b), unsigned or signed (sign-bit
                        // complement). i8 leaves Z = (a == b) too.
                        self.emit_cmp_c(&ic.a, &ic.b, ic.ty, signed);
                        // The i16 borrow chain ends with a byte-level Z;
                        // full equality needs the XOR accumulation, which
                        // preserves C.
                        if need_z && ic.ty.bytes() == 2 {
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
                self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
            }
            Inst::Call(c) => self.emit_call(&c.dst, c.ty, &c.func, &c.args),
            _ => panic!("isel: unsupported instruction for milestone 2"),
        }
    }

    fn emit_terminator(&mut self, t: &Inst, labels: &HashMap<String, String>) {
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
            Inst::Ret(None) => self.emit("    RETURN".to_string()),
            Inst::Ret(Some((ty, v))) => {
                // Copy the value into the fixed retval slots, then RETURN.
                for i in 0..ty.bytes() {
                    self.emit_load_byte(v, i);
                    self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + u16::from(i)));
                }
                self.emit("    RETURN".to_string());
            }
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }
}

/// Select instructions for the whole module, producing PIC14 assembly text.
///
/// `addrs` is the complete address map from `alloc`: globals by name, locals
/// by `{func}::{name}` (IR value names without `%`). isel does no slot
/// allocation — every value's address is read from the map. The icmp scratch
/// byte and the two retval bytes live in fixed common RAM (scratch `0x70`,
/// retval `0x71`/`0x72`): bank-independent, never used by locals (M3), so no
/// BANKSEL is ever needed for them.
pub fn select(m: &Module, addrs: &HashMap<String, u16>) -> String {
    // The icmp scratch byte and the two retval bytes are fixed common-RAM
    // constants (bank-independent, common RAM 0x70-0x7F is never used by
    // locals, so no collision).
    let scratch: u16 = 0x70;
    let retval_lo: u16 = 0x71;
    let mut out = vec![
        "; pic8 -- integer spine milestone 2 (isel)".to_string(),
        "    list p=16f877a".to_string(),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR    equ 0x04".to_string(),
        "INDF   equ 0x00".to_string(),
        "PCL    equ 0x02".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    // Phase-3 pointers: collect every GEP and resolve its chain eagerly to a
    // folded `(base, k, terms)`, keyed `{func}::{reg}` like every other
    // local. Seeds first: a byval param slot IS the struct copy
    // (Slot(name, false)); an sret param slot holds the target address
    // (Slot(name, true)); an alloca defines its own buffer slot
    // (Slot(name, false)). Gep itself is virtual — it emits nothing.
    let mut geps: HashMap<String, Gep> = HashMap::new();
    let mut resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
    for f in &m.funcs {
        for p in &f.params {
            if p.byval.is_some() {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), false), 0, Vec::new()),
                );
            } else if p.sret {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), true), 0, Vec::new()),
                );
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Gep(g) => {
                        geps.insert(ssa_key(&f.name, &g.dst), g.clone());
                    }
                    Inst::Alloca(a) => {
                        resolved.insert(
                            ssa_key(&f.name, &a.dst),
                            (Base::Slot(a.dst.clone(), false), 0, Vec::new()),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    // Chain folding (fixpoint scan): a GEP whose base is a `Reg` folds in
    // that reg's own resolved entry — `k` adds, terms concatenate
    // (inner-first: terms_inner + terms_outer) — until the base is a Global
    // or a seeded Slot. A base that is neither a gep nor a seed panics; a
    // pass that makes no progress with unresolved geeps left is a cycle and
    // panics loudly.
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut pending: Vec<(String, Gep)> = geps
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{fname}::")))
            .map(|(k, g)| (k.clone(), g.clone()))
            .collect();
        let mut progressed = true;
        while progressed {
            progressed = false;
            let mut rest = Vec::new();
            for (key, g) in pending {
                match &g.base {
                    GepBase::Global(name) => {
                        assert!(
                            !resolved.contains_key(&key),
                            "isel: duplicate definition of pointer reg {key}"
                        );
                        resolved.insert(key, (Base::Global(name.clone()), g.k, g.terms.clone()));
                        progressed = true;
                    }
                    GepBase::Reg(r) => {
                        let rkey = ssa_key(&fname, r);
                        if let Some((b, kk, tt)) = resolved.get(&rkey).cloned() {
                            assert!(
                                !resolved.contains_key(&key),
                                "isel: duplicate definition of pointer reg {key}"
                            );
                            let mut terms = tt.clone();
                            terms.extend(g.terms.clone());
                            let k = g.k.checked_add(kk).unwrap_or_else(|| {
                                panic!("isel: gep offset overflow in {key}")
                            });
                            resolved.insert(key, (b, k, terms));
                            progressed = true;
                        } else if geps.contains_key(&rkey) {
                            rest.push((key, g)); // may resolve on a later pass
                        } else {
                            panic!(
                                "isel: no gep for pointer %{r} (chain base missing, key {rkey})"
                            );
                        }
                    }
                }
            }
            pending = rest;
            if !progressed && !pending.is_empty() {
                let names: Vec<&str> = pending.iter().map(|(k, _)| k.as_str()).collect();
                panic!("isel: cyclic gep chain involving {names:?}");
            }
        }
    }
    // Fresh-label counter at module scope: labels are file-scoped in the
    // single `.asm` output, so it must not reset per function.
    let mut tmp = 0u32;
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            resolved: &resolved,
            scratch,
            retval_lo,
            cur_func: &f.name,
            tmp: &mut tmp,
            out: Vec::new(),
        };
        // Block label scheme: the entry block uses the bare function name
        // (so CALLs and GOTOs resolve to it); every other block is
        // `{func}_L{label}`. The entry block's label is emitted by the block
        // loop below — no standalone function label here, or `main:` /
        // `add:` would be defined twice and gpasm would reject the file.
        let mut labels: HashMap<String, String> = HashMap::new();
        for (i, b) in f.blocks.iter().enumerate() {
            let lbl = if i == 0 {
                f.name.clone()
            } else {
                format!("{}_L{}", f.name, b.label)
            };
            labels.insert(b.label.clone(), lbl);
        }
        // phi elimination: for each predecessor block, the copies that must
        // run before it branches into the merge block.
        let mut phi_copies: HashMap<String, Vec<(String, Ty, Val)>> = HashMap::new();
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Phi(p) = i {
                    for (val, pred) in &p.incoming {
                        phi_copies
                            .entry(pred.clone())
                            .or_default()
                            .push((p.dst.clone(), p.ty, val.clone()));
                    }
                }
            }
        }
        for b in &f.blocks {
            g.emit(format!("{}:", labels[&b.label]));
            let mut terminator = None;
            for i in &b.insts {
                match i {
                    Inst::Phi(_) => {} // eliminated; copies emitted at pred ends
                    Inst::Br(_) | Inst::BrCond(_) | Inst::Ret(_) => terminator = Some(i),
                    _ => g.emit_inst(i),
                }
            }
            if let Some(copies) = phi_copies.get(&b.label) {
                g.emit(format!("    ; phi copies for pred {0}", labels[&b.label]));
                // Emit the predecessor's copies in dependency order so a copy
                // never overwrites a slot a later copy still needs to read.
                // Chains (%a <- %b, %b <- %c) emit c->b then b->a. If the
                // copies form a cycle (%a = phi [%b,P], %b = phi [%a,P] —
                // clang -O1 emits this for loop-carried swaps), no ordering
                // works without a temp register, so panic loudly rather than
                // silently miscompile.
                let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
                    .iter()
                    .map(|(dst, ty, val)| {
                        let da = g.slot_addr(g.cur_func, dst);
                        let src = match val {
                            Val::Reg(r) => Some(g.slot_addr(g.cur_func, r)),
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
                        // Blocked while an un-emitted sibling (j != i) writes
                        // this copy's source slot (sibling destination ==
                        // this copy's source).
                        let blocked = match src {
                            Some(s) => {
                                (0..n).any(|j| !emitted[j] && j != i && pending[j].0 == *s)
                            }
                            None => false,
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
            if let Some(t) = terminator {
                g.emit_terminator(t, &labels);
            }
        }
        g.emit("".to_string());
        out.extend(g.out);
    }
    // Const (flash) globals become RETLW tables, emitted after the
    // functions so the CALLs above resolve. `__read_<name>` adds the
    // table's low address to the index in W and jumps into it via PCL; the
    // RETLW of the selected byte returns with W = the byte.
    let mut consts: Vec<&ir::Global> = m.globals.iter().filter(|g| g.is_const).collect();
    consts.sort_by_key(|g| g.name.clone());
    for g in consts {
        out.push(format!("__read_{}:", g.name));
        out.push(format!("    ADDLW LOW({})", g.name));
        out.push("    MOVWF PCL".to_string());
        out.push(format!("{}:", g.name));
        assert!(
            !g.bytes.is_empty(),
            "isel: const @{} has no table bytes",
            g.name
        );
        for b in &g.bytes {
            out.push(format!("    RETLW 0x{b:02X}"));
        }
        out.push("".to_string());
    }
    out.push("__start:".to_string());
    out.push("    CALL main".to_string());
    out.push("    SLEEP".to_string());
    out.push("".to_string());
    out.push("    end".to_string());
    out.join("\n")
}

/// Parse an alloc-produced address-map text into `HashMap<String, u16>`:
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become map
/// entries (locals keyed `{func}::{name}`); `const <name>` lines list flash
/// globals, which have no RAM address, so they are accepted and skipped —
/// isel reads their bytes from the `Module`, never from a RAM slot.
pub fn parse_map(text: &str) -> HashMap<String, u16> {
    let mut addrs = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().expect("map entry");
        match kw {
            "const" => {
                // Flash global: no RAM address; nothing to record.
            }
            "global" => {
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("isel: bad address in map line: {line}"));
                addrs.insert(name, addr);
            }
            "local" => {
                let func = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("isel: bad address in map line: {line}"));
                addrs.insert(format!("{func}::{name}"), addr);
            }
            _ => panic!("isel: unexpected map line: {line}"),
        }
    }
    addrs
}
