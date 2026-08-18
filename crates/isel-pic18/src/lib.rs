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

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Ret(None) => self.emit("    RETURN".to_string()),
            other => panic!("isel-pic18: unsupported instruction for P2 (so far): {other:?}"),
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
        // Block/label emission arrives in Task 12 (the index-based scheme
        // `isel::select` uses: the first block gets the bare function name,
        // every other block gets `{func}_L{label}`); this skeleton only
        // walks each block's instructions, so no label is emitted yet —
        // including for the entry block.
        for b in &f.blocks {
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
