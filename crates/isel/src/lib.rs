//! `isel` — instruction selection for the integer spine.
//!
//! Milestone-2 subset: lowers `load`/`store`, `add` and `and` (i8 and i16),
//! `zext`/`trunc`, and eliminates `phi` by copying each predecessor's
//! incoming value into the phi destination at the end of the predecessor
//! block, before its terminator. Any other instruction or binop panics.

use ir::{BinOp, Inst, Module, Ty, Val};
use std::collections::HashMap;

const COMMON_START: u8 = 0x70;
const BANK0_START: u8 = 0x25;

/// Per-function codegen state: the SSA slot map, the next free GPR address,
/// and the lines emitted for the current function.
struct Gen<'m> {
    addrs: &'m HashMap<String, u8>,
    slots: HashMap<String, u8>,
    next: u8,
    out: Vec<String>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    /// Assign (or return the existing) GPR base address for an SSA value of
    /// `ty`. An i16 occupies two consecutive slots (lo at `a`, hi at `a+1`),
    /// so two bytes are reserved. Common RAM `0x70`→`0x7F` is consumed first;
    /// once exhausted, bank-0 GPRs from `0x25` are used. This is a
    /// milestone-2 approximation; overlay allocation and spill handling land
    /// in a later milestone.
    fn slot(&mut self, name: &str, ty: Ty) -> u8 {
        if let Some(&a) = self.slots.get(name) {
            return a;
        }
        // A multi-byte value must fit entirely in common RAM (0x70..=0x7F).
        // If it would straddle the boundary (e.g. an i16 with its lo at
        // 0x7F would place the hi at 0x80, aliasing bank-1 INDF), spill the
        // whole value to bank-0 GPRs first. Mirrors spike alloc_slot's
        // `common + n - 1 <= COMMON_END` fit check.
        let n = ty.bytes();
        if self.next + n - 1 > 0x7F {
            self.next = BANK0_START;
        }
        let a = self.next;
        self.next = self.next.wrapping_add(n);
        self.slots.insert(name.to_string(), a);
        a
    }

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> u8 {
        match v {
            Val::Reg(r) => *self
                .slots
                .get(r)
                .unwrap_or_else(|| panic!("isel: no slot for %{r}")),
            Val::Global(g) => *self
                .addrs
                .get(g)
                .unwrap_or_else(|| panic!("isel: no address for @{g}")),
            Val::Const(k) => {
                assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
                *k as u8
            }
        }
    }

    /// Resolve a memory pointer ("@name" global or "%name" slot) to an address.
    fn ptr_addr(&self, p: &str) -> u8 {
        if let Some(g) = p.strip_prefix('@') {
            *self
                .addrs
                .get(g)
                .unwrap_or_else(|| panic!("isel: no address for @{g}"))
        } else {
            let name = p.trim_start_matches('%');
            *self
                .slots
                .get(name)
                .unwrap_or_else(|| panic!("isel: no slot for %{name}"))
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
                self.emit(format!("    MOVF 0x{:02X}, W", a + idx));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
                self.emit(format!("    MOVF 0x{:02X}, W", a + idx));
            }
        }
    }

    /// Copy `val` (width `ty`) into the slot starting at `dst`.
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u8) {
        for i in 0..ty.bytes() {
            self.emit_load_byte(val, i);
            self.emit(format!("    MOVWF 0x{:02X}", dst + i));
        }
    }

    /// `d = a + b` for i16 (either operand may be a register; at most one a
    /// const). Low byte adds, then the high byte adds with the carry from the
    /// low byte folded in via BTFSC/ADDLW.
    fn emit_add16(&mut self, a: &Val, b: &Val, dst: u8) {
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
    fn emit_and16(&mut self, a: &Val, b: &Val, dst: u8) {
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

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel: only i8/i16 loads supported");
                let (src, dst) = (self.ptr_addr(&l.ptr), self.slot(&l.dst, l.ty));
                for k in 0..l.ty.bytes() {
                    self.emit(format!("    MOVF 0x{:02X}, W", src + k));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + k));
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel: only i8/i16 stores supported");
                let dst = self.ptr_addr(&s.ptr);
                self.emit_move_val_to_slot(&s.val, s.ty, dst);
            }
            Inst::Bin(b) => {
                assert!(b.ty != Ty::I1, "isel: only i8/i16 binops supported");
                let da = self.slot(&b.dst, b.ty);
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
                let da = self.slot(&z.dst, z.to);
                for i in 0..z.from.bytes() {
                    self.emit_load_byte(&z.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + i));
                }
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit(format!("    CLRF 0x{:02X}", da + i));
                }
            }
            Inst::Trunc(t) => {
                assert!(
                    t.from.bytes() > t.to.bytes(),
                    "isel: trunc must narrow"
                );
                let da = self.slot(&t.dst, t.to);
                for i in 0..t.to.bytes() {
                    self.emit_load_byte(&t.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + i));
                }
            }
            _ => panic!("isel: unsupported instruction for milestone 2"),
        }
    }

    fn emit_terminator(&mut self, t: &Inst, labels: &HashMap<String, String>) {
        match t {
            Inst::Br(br) => {
                let l = &labels[&br.target];
                self.emit(format!("    GOTO {l}"));
            }
            Inst::Ret(None) => self.emit("    RETURN".to_string()),
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }
}

/// Select instructions for the whole module, producing PIC14 assembly text.
///
/// `addrs` maps global names to their bank-0 GPR addresses (from `alloc`).
/// SSA destinations are assigned fresh addresses per function: common RAM
/// (`0x70`→`0x7F`) first, then bank-0 GPRs from `0x25`.
pub fn select(m: &Module, addrs: &HashMap<String, u8>) -> String {
    let mut out = vec![
        "; pic8 -- integer spine milestone 2 (isel)".to_string(),
        "    list p=16f877a".to_string(),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    for f in &m.funcs {
        let mut g = Gen {
            addrs,
            slots: HashMap::new(),
            next: COMMON_START,
            out: Vec::new(),
        };
        g.emit(format!("{0}:", f.name));
        // Block label scheme: the entry block uses the bare function name;
        // every other block is `{func}_L{label}`.
        let mut labels: HashMap<String, String> = HashMap::new();
        for (i, b) in f.blocks.iter().enumerate() {
            let lbl = if i == 0 {
                f.name.clone()
            } else {
                format!("{}_L{}", f.name, b.label)
            };
            labels.insert(b.label.clone(), lbl);
        }
        // Reserve slots for every phi destination up front so the elimination
        // copies land at stable, non-overlapping addresses regardless of the
        // order in which the predecessor blocks are lowered.
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Phi(p) = i {
                    g.slot(&p.dst, p.ty);
                }
            }
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
                for (dst, ty, val) in copies {
                    let da = g.slot(dst, *ty);
                    g.emit_move_val_to_slot(val, *ty, da);
                }
            }
            if let Some(t) = terminator {
                g.emit_terminator(t, &labels);
            }
        }
        g.emit("".to_string());
        out.extend(g.out);
    }
    out.push("__start:".to_string());
    out.push("    CALL main".to_string());
    out.push("    SLEEP".to_string());
    out.push("".to_string());
    out.push("    end".to_string());
    out.join("\n")
}
