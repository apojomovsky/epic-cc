//! `isel` — instruction selection for the integer spine.
//!
//! Milestone-2 subset: lowers `load`/`store`, `add` and `and` (i8 and i16),
//! `zext`/`trunc`, `call` (arg copies into the callee's param slots + retval
//! copy), `ret` with and without a value, and eliminates `phi` by copying each
//! predecessor's incoming value into the phi destination at the end of the
//! predecessor block, before its terminator. Any other instruction or binop
//! panics.

use ir::{BinOp, Inst, Module, Ty, Val};
use std::collections::HashMap;

const COMMON_START: u8 = 0x70;

/// Module-wide SSA slot key: `{func}::{name}`. Every value of every function
/// lives in one shared map with one shared allocator counter, so a callee's
/// param slots can never collide with the caller's live slots (the
/// milestone-1 per-function maps restarted at 0x70 for each function and did
/// collide across CALL boundaries).
fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// Assign (or return the existing) GPR base address for SSA value `key` of
/// `ty` in the module-wide slot map. An i16 occupies two consecutive slots
/// (lo at `a`, hi at `a+1`), so two bytes are reserved. Common RAM
/// `0x70`→`0x7F` is consumed first; once exhausted, bank-0 GPRs from
/// `bank0_start` are used. This is a milestone-2 approximation; overlay
/// allocation and spill handling land in a later milestone.
fn alloc_slot(
    slots: &mut HashMap<String, u8>,
    next: &mut u8,
    key: &str,
    ty: Ty,
    scratch: u8,
    retval_lo: u8,
    bank0_start: u8,
) -> u8 {
    if let Some(&a) = slots.get(key) {
        return a;
    }
    // A multi-byte value must fit entirely in common RAM (0x70..=0x7F).
    // If it would straddle the boundary (e.g. an i16 with its lo at
    // 0x7F would place the hi at 0x80, aliasing bank-1 INDF), spill the
    // whole value to bank-0 GPRs first. Mirrors spike alloc_slot's
    // `common + n - 1 <= COMMON_END` fit check.
    let n = ty.bytes();
    if *next + n - 1 > 0x7F {
        *next = bank0_start;
    }
    // The fixed icmp scratch byte and the two retval bytes live just past
    // the globals. Never let a slot's lo or hi byte land on any of them, or
    // an icmp / a callee's Ret in the same function would silently corrupt
    // that slot. These are true overlap tests, not >=-only boundary checks:
    // the allocator's initial counter (COMMON_START) can already sit inside a
    // reserved region (e.g. end_of_globals = 0x6E puts retval_hi at 0x70 ==
    // COMMON_START), in which case the candidate slot overlaps the region even
    // though its start is past the reserved lo. wrapping_add keeps near-0xFF
    // u8 counters from wrapping in debug builds.
    if *next < scratch.wrapping_add(1) && next.wrapping_add(n) > scratch {
        *next = scratch.wrapping_add(1);
    }
    if *next < retval_lo.wrapping_add(2) && next.wrapping_add(n) > retval_lo {
        *next = retval_lo.wrapping_add(2);
    }
    let a = *next;
    *next = next.wrapping_add(n);
    slots.insert(key.to_string(), a);
    a
}

/// Per-function codegen state. The SSA slot map and its allocator counter are
/// shared across the whole module (behind `&mut`), so slots are keyed
/// `{func}::{name}`; `cur_func` selects the current function's entries.
struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u8>,
    slots: &'m mut HashMap<String, u8>,
    next: &'m mut u8,
    scratch: u8,
    retval_lo: u8,
    bank0_start: u8,
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

    /// Assign (or return the existing) GPR base address for the current
    /// function's SSA value `name` of `ty`, via the module-wide shared map.
    fn slot(&mut self, name: &str, ty: Ty) -> u8 {
        let key = ssa_key(self.cur_func, name);
        alloc_slot(
            self.slots,
            self.next,
            &key,
            ty,
            self.scratch,
            self.retval_lo,
            self.bank0_start,
        )
    }

    /// Resolve `{func}::{name}` to its base byte address (lo for multi-byte).
    fn slot_addr(&self, func: &str, name: &str) -> u8 {
        *self
            .slots
            .get(&ssa_key(func, name))
            .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}"))
    }

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> u8 {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
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
            self.slot_addr(self.cur_func, name)
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

    /// W ^= byte `idx` of `val`.
    fn emit_xor_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    XORLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    XORWF 0x{:02X}, W", a + idx));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
                self.emit(format!("    XORWF 0x{:02X}, W", a + idx));
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
        let da = self.slot(dst, ty);
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
            let da = self.slot(d, t);
            for i in 0..t.bytes() {
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo + i));
                self.emit(format!("    MOVWF 0x{:02X}", da + i));
            }
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
            Inst::Icmp(ic) => {
                assert!(
                    ic.pred == "eq",
                    "isel: only eq icmp supported (got {:?})",
                    ic.pred
                );
                // XOR-based compare sets Z = (a == b); materialize the i1.
                self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                let da = self.slot(&ic.dst, Ty::I1);
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
                    self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + i));
                }
                self.emit("    RETURN".to_string());
            }
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }
}

/// Select instructions for the whole module, producing PIC14 assembly text.
///
/// `addrs` maps global names to their bank-0 GPR addresses (from `alloc`).
/// The layout follows the spike: the icmp scratch byte sits at
/// `end_of_globals` (max global addr + size), the retval slots at
/// `end_of_globals+1/+2`, and bank-0 GPR overflow starts at
/// `end_of_globals+3` (probe: globals 0x20/0x21 → scratch 0x22, retval
/// 0x23/0x24, bank0 0x25). SSA destinations are assigned fresh addresses in a
/// single module-wide map keyed `{func}::{name}`: common RAM (`0x70`→`0x7F`)
/// first, then bank-0 GPRs.
pub fn select(m: &Module, addrs: &HashMap<String, u8>) -> String {
    // end_of_globals = max over the address map of (global addr + size). The
    // scratch and retval slots live immediately after it so a large global
    // array can never collide with them. Globals absent from the map (e.g.
    // const/failed modules) don't move the end.
    let end_of_globals = m
        .globals
        .iter()
        .fold(0x20u8, |end, g| match addrs.get(&g.name) {
            Some(&a) => end.max(a.wrapping_add(g.ty.bytes())),
            None => end,
        });
    // bank0_start must stay within bank-0 GPR range (<= 0x7F). A degenerate
    // address map with a global ending at >= 0xFC would wrap the u8 add in
    // release builds and silently miscompile; guard it loudly.
    debug_assert!(end_of_globals <= 0x7C, "isel: globals exceed bank-0 layout");
    let scratch = end_of_globals;
    let retval_lo = end_of_globals + 1;
    let bank0_start = end_of_globals + 3;
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
    // Fresh-label counter at module scope: labels are file-scoped in the
    // single `.asm` output, so it must not reset per function.
    let mut tmp = 0u32;
    // Module-wide SSA slot map and allocator counter, shared by every
    // function: each value gets a distinct address, so the callee's param
    // slots can never overlap the caller's live slots (the milestone-1
    // per-function maps collided across CALL boundaries).
    let mut slots: HashMap<String, u8> = HashMap::new();
    let mut next: u8 = COMMON_START;
    // Reserve every function's params first (spike order) so a CALL's arg
    // copies always find a stable callee param address.
    for f in &m.funcs {
        for (ty, name) in &f.params {
            let key = ssa_key(&f.name, name);
            alloc_slot(&mut slots, &mut next, &key, *ty, scratch, retval_lo, bank0_start);
        }
    }
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            slots: &mut slots,
            next: &mut next,
            scratch,
            retval_lo,
            bank0_start,
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
        // Reserve slots for every instruction destination up front so the
        // elimination copies and forward references (a value defined in a
        // later block but read by an earlier one, e.g. a call result feeding
        // a trunc in the merge block) land at stable, non-overlapping
        // addresses regardless of the order in which the blocks are lowered.
        // Phi destinations are reserved first — that preserves the layout
        // the unit tests document (phi dst first at 0x70, then loads etc. in
        // block order). The `Ty` used for each destination must match the
        // emit path below exactly, since it fixes the reserved width.
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Phi(p) = i {
                    g.slot(&p.dst, p.ty);
                }
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Load(l) => {
                        g.slot(&l.dst, l.ty);
                        ()
                    }
                    Inst::Bin(b) => {
                        g.slot(&b.dst, b.ty);
                        ()
                    }
                    Inst::Zext(z) => {
                        g.slot(&z.dst, z.to);
                        ()
                    }
                    Inst::Trunc(t) => {
                        g.slot(&t.dst, t.to);
                        ()
                    }
                    Inst::Icmp(ic) => {
                        g.slot(&ic.dst, Ty::I1);
                        ()
                    }
                    Inst::Select(s) => {
                        g.slot(&s.dst, s.ty);
                        ()
                    }
                    Inst::Call(c) => {
                        if let Some(d) = &c.dst {
                            let t = c.ty.expect("isel: valued call must carry a type");
                            g.slot(d, t);
                        }
                        ()
                    }
                    _ => {}
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
                // Emit the predecessor's copies in dependency order so a copy
                // never overwrites a slot a later copy still needs to read.
                // Chains (%a <- %b, %b <- %c) emit c->b then b->a. If the
                // copies form a cycle (%a = phi [%b,P], %b = phi [%a,P] —
                // clang -O1 emits this for loop-carried swaps), no ordering
                // works without a temp register, so panic loudly rather than
                // silently miscompile.
                let pending: Vec<(u8, Option<u8>, Ty, Val)> = copies
                    .iter()
                    .map(|(dst, ty, val)| {
                        let da = g.slot(dst, *ty);
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
    out.push("__start:".to_string());
    out.push("    CALL main".to_string());
    out.push("    SLEEP".to_string());
    out.push("".to_string());
    out.push("    end".to_string());
    out.join("\n")
}
