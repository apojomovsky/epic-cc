//! `isel` — instruction selection for the integer spine.
//!
//! Milestone-2 subset: lowers `load`/`store`, `add` and `and` (i8 and i16),
//! `zext`/`trunc`, `call` (arg copies into the callee's param slots + retval
//! copy), `ret` with and without a value, and eliminates `phi` by copying each
//! predecessor's incoming value into the phi destination at the end of the
//! predecessor block, before its terminator. Any other instruction or binop
//! panics.
//!
//! Phase-3 pointers/const: `gep` defines a virtual pointer, lowered at each
//! `load`/`store` use. A pointer into a RAM array accesses `FSR`/`INDF`
//! (`base_lo + offset`); a pointer into a const (flash) global loads via
//! `CALL __read_<name>` — a RETLW table emitted after the functions. A store
//! through a pointer into a const global panics (ROM is not writable).
//!
//! Every value's address comes from the caller-supplied address map: globals
//! by name, locals by `{func}::{name}` (IR value names without `%`). isel
//! performs no slot allocation; it trusts the map (from `alloc`'s overlay
//! layout) and panics loudly if a value is missing from it.

use ir::{BinOp, Inst, Module, Ty, Val};
use std::collections::HashMap;

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// Per-function codegen state. All addresses come from the module-wide map;
/// `cur_func` selects the current function's local entries.
struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    /// Every GEP in the module, keyed `{func}::{dst}` -> (base global,
    /// offset). `gep` itself emits nothing; each `load`/`store` through a
    /// pointer reg lowers the pointer at its use.
    geps: &'m HashMap<String, (String, Val)>,
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

    /// A GEP-created pointer `%r` -> its (base global, offset). Pointers
    /// come only from `gep` in this milestone; anything else is a missing
    /// slot and panics loudly.
    fn gep_for(&self, r: &str) -> (String, Val) {
        let key = ssa_key(self.cur_func, r);
        self.geps
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
    }

    /// `FSR = base_lo + offset`: W = offset low byte, W += the RAM global's
    /// byte address, then FSR = W. Bank-0 arrays only — the base must fit
    /// the 8-bit ADDLW literal.
    fn emit_fsr_setup(&mut self, base: &str, offset: &Val) {
        let base_lo = self.global_addr(base);
        assert!(
            base_lo <= 0xFF,
            "isel: indirect base @{base} at 0x{base_lo:02X} needs a banked FSR setup"
        );
        self.emit_load_byte(offset, 0); // W = offset low byte
        self.emit(format!("    ADDLW 0x{base_lo:02X}")); // W = base + offset
        self.emit("    MOVWF FSR".to_string());
    }

    /// `dst = ram[base + offset]` via FSR/INDF (i8 only, like the spike).
    fn emit_ram_indirect_load(&mut self, base: &str, offset: &Val, dst: u16, ty: Ty) {
        assert!(ty.bytes() == 1, "isel: multi-byte indirect load not supported");
        self.emit_fsr_setup(base, offset);
        self.emit("    MOVF INDF, W".to_string());
        self.emit(format!("    MOVWF 0x{dst:02X}"));
    }

    /// `ram[base + offset] = val` via FSR/INDF (i8 only, like the spike).
    fn emit_ram_indirect_store(&mut self, base: &str, offset: &Val, val: &Val, ty: Ty) {
        assert!(ty.bytes() == 1, "isel: multi-byte indirect store not supported");
        self.emit_fsr_setup(base, offset);
        self.emit_load_byte(val, 0);
        self.emit("    MOVWF INDF".to_string());
    }

    /// `dst = flash_table[offset]`: W = index, CALL __read_<base> (a RETLW
    /// table), W = the selected byte -> dst. i8 only, like the spike.
    fn emit_const_read(&mut self, base: &str, offset: &Val, dst: u16, ty: Ty) {
        assert!(ty.bytes() == 1, "isel: multi-byte const load not supported");
        self.emit_load_byte(offset, 0); // W = offset (index)
        self.emit(format!("    CALL __read_{base}"));
        self.emit(format!("    MOVWF 0x{dst:02X}"));
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

    /// `d = a & b` for i16: an AND per byte.
    fn emit_and16(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: and16 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg));
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    ANDWF 0x{ra:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit(format!("    ANDWF 0x{:02X}, W", ra + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVF 0x{ra:02X}, W"));
                self.emit(format!("    ANDLW 0x{lo:02X}"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", ra + 1));
                self.emit(format!("    ANDLW 0x{hi:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(_) => panic!("isel: and16 with a global operand"),
        }
    }

    /// `dst = call func(args)`: copy each arg into the callee's
    /// `{func}::{param}` slots, `CALL func`, then copy the retval slots
    /// (`retval_lo`/`retval_lo+1`) into `dst`. Void calls skip the retval
    /// copy. Mirrors spike emit_call.
    fn emit_call(&mut self, dst: &Option<String>, ty: Option<Ty>, func: &str, args: &[(Ty, Val)]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        for (i, (aty, val)) in args.iter().enumerate() {
            let pname = &callee.params[i].1;
            let pa = self.slot_addr(func, pname);
            self.emit_move_val_to_slot(val, *aty, pa);
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
                    // A GEP-created pointer: const bases read via a RETLW
                    // table; RAM bases via FSR/INDF.
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global or %reg", l.ptr)
                    });
                    let (base, offset) = self.gep_for(r);
                    if self.global_is_const(&base) {
                        self.emit_const_read(&base, &offset, dst, l.ty);
                    } else {
                        self.emit_ram_indirect_load(&base, &offset, dst, l.ty);
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
                    let (base, offset) = self.gep_for(r);
                    assert!(
                        !self.global_is_const(&base),
                        "isel: store to const (flash) global @{base}"
                    );
                    self.emit_ram_indirect_store(&base, &offset, &s.val, s.ty);
                }
            }
            Inst::Gep(_) => {} // virtual: lowered at each load/store use
            Inst::Bin(b) => {
                assert!(b.ty != Ty::I1, "isel: only i8/i16 binops supported");
                let da = self.slot_addr(self.cur_func, &b.dst);
                match (b.op, b.ty) {
                    (BinOp::Add, Ty::I16) => self.emit_add16(&b.a, &b.b, da),
                    (BinOp::And, Ty::I16) => self.emit_and16(&b.a, &b.b, da),
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
                assert!(
                    ic.pred == "eq",
                    "isel: only eq icmp supported (got {:?})",
                    ic.pred
                );
                // XOR-based compare sets Z = (a == b); materialize the i1.
                self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                let da = self.slot_addr(self.cur_func, &ic.dst);
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    BTFSC STATUS, 2 ; Z".to_string());
                self.emit("    MOVLW 0x01".to_string());
                self.emit(format!("    MOVWF 0x{da:02X}"));
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
    // Phase-3 pointers: collect every GEP so a load/store through a pointer
    // reg can find its (base global, offset). Keyed `{func}::{dst}` like
    // every other local. Gep itself is virtual — it emits nothing.
    let mut geps: HashMap<String, (String, Val)> = HashMap::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Gep(g) = i {
                    geps.insert(
                        ssa_key(&f.name, &g.dst),
                        (g.base.clone(), g.offset.clone()),
                    );
                }
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
            geps: &geps,
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
