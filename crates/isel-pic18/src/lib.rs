//! `isel-pic18` — instruction selection for the PIC18 integer spine (P2).
//! Same scope as `isel`'s milestones 2-6 (`Load`/`Store`/`Bin`(add/sub/
//! and/or/xor)/`Icmp`/`Zext`/`Sext`/`Trunc`/`Select`/`Call`/`Br`/`BrCond`/
//! `Phi`/`Ret`) — see docs/superpowers/plans/2026-08-18-pic18-port-p2.md.
//! A separate crate from `isel` per docs/29-pic18-port-design.md §2 D-1:
//! the instruction sets differ enough that sharing would leak an
//! abstraction, and PIC14's working code must never be at risk from a
//! PIC18 edit.

use std::collections::HashMap;

use device::Device;
use ir::{Inst, Module, Ty, Val};
use iselcore::{ssa_key, Slot};

struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
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

    /// The `,A`/`,B` operand components `(a, f)` for a physical address
    /// used by a `W`-routing instruction (`ADDWF`/`SUBWF`/.../`CPFSxx`),
    /// emitting `MOVLB` first if the tracked `BSR` doesn't already match.
    /// `MOVFF`-based plain copies (`emit_copy_byte`, below) never call
    /// this — they take full 12-bit addresses directly and need no bank
    /// bit at all, which is why `Load`/`Store`/`Phi`-copies/`Call`-arg-
    /// copies (Tasks 4, 11, 12, 13) never touch `BSR`.
    fn operand(&mut self, addr: u16) -> (u16, u16) {
        if addr < 0x60 {
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

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Ret(None) => self.emit("    RETURN".to_string()),
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel-pic18: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                let src = self.global_addr(l.ptr.strip_prefix('@').unwrap_or_else(|| {
                    panic!("isel-pic18: P2 only supports loads from @global (no pointers yet)")
                }));
                for k in 0..l.ty.bytes() {
                    self.emit_copy_byte(src + u16::from(k), dst + u16::from(k));
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel-pic18: only i8/i16 stores supported");
                let dst = self.global_addr(s.ptr.strip_prefix('@').unwrap_or_else(|| {
                    panic!("isel-pic18: P2 only supports stores to @global (no pointers yet)")
                }));
                self.emit_move_val_to_slot(&s.val, s.ty, dst);
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
                assert_eq!(c.ty.bytes(), 1, "isel-pic18: only i8 Icmp implemented so far (Task 9 adds i16)");
                self.emit_icmp_byte(c.a.clone(), c.b.clone(), &c.pred, &c.dst);
            }
            other => panic!("isel-pic18: unsupported instruction for P2 (so far): {other:?}"),
        }
    }

    /// `dst = (a <pred> b) ? 1 : 0` for one byte, via `a - b` (SUBWF: f=a,
    /// W=b beforehand, d=W so `a`'s slot is untouched) and a flag-based
    /// branch. C/Z/N/OV follow PIC18's standard (ARM-style) condition-code
    /// semantics — C=1 means "no borrow" (a>=b unsigned) — already relied
    /// on by P1's `Pic18::sub_flags`.
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
        self.emit_load_w(&b, 0);
        let av = self.val_addr(&a).direct();
        let (acc, af) = self.operand(av);
        let bank = if acc == 0 { "A" } else { "B" };
        self.emit(format!("    SUBWF 0x{af:03X},W,{bank}")); // W = a - b

        let l_true = self.fresh_label();
        let l_done = self.fresh_label();
        match pred {
            "eq" => self.emit(format!("    BZ {l_true}")),
            "ne" => self.emit(format!("    BNZ {l_true}")),
            "ult" => self.emit(format!("    BNC {l_true}")),
            "uge" => self.emit(format!("    BC {l_true}")),
            "ugt" => {
                // C=1 AND Z=0
                let l_false = self.fresh_label();
                self.emit(format!("    BNC {l_false}"));
                self.emit(format!("    BNZ {l_true}"));
                self.emit(format!("{l_false}:"));
            }
            "ule" => {
                // C=0 OR Z=1
                self.emit(format!("    BNC {l_true}"));
                self.emit(format!("    BZ {l_true}"));
            }
            other => panic!("isel-pic18: icmp predicate {other} not yet implemented (Task 8)"),
        }
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    BRA {l_done}"));
        self.emit(format!("{l_true}:"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("{l_done}:"));
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
    for f in &m.funcs {
        let mut g = Gen {
            m,
            addrs,
            retval_lo: common_lo,
            bsr: None,
            cur_func: &f.name,
            tmp: &mut tmp,
            out: Vec::new(),
        };
        // Full multi-block label emission (every non-entry block getting
        // `{func}_L{label}`) arrives in Task 12. But `__start` already
        // unconditionally emits `call main` above, and Task 6 is the first
        // task whose tests actually assemble+simulate the generated code
        // (rather than just checking the asm text), which exposed that gap:
        // without a `main:` label, `call main` is an unresolved symbol and
        // assembly fails outright. So the entry (first) block gets its
        // label — the bare function name — here now, matching the target
        // scheme's rule for the first block; the "every other block" half
        // of the scheme is still deferred to Task 12.
        for (bi, b) in f.blocks.iter().enumerate() {
            if bi == 0 {
                g.emit(format!("{}:", f.name));
            }
            for inst in &b.insts {
                g.emit_inst(inst);
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
        let mut tmp = 0u32;
        let l1 = {
            let mut g = Gen {
                m: &m,
                addrs: &addrs,
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
