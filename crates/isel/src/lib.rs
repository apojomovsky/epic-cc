//! `isel` — straight-line 8-bit instruction selection.
//!
//! Milestone-1 subset: translates the canonical IR (`ir::Module`) into PIC14
//! assembly text (`.asm`), covering only `load`, `store`, `add` (const and
//! reg forms), and `ret void`. Any other instruction or binop panics loudly.

use ir::{BinOp, Inst, Module, Val};
use std::collections::HashMap;

const COMMON_START: u8 = 0x70;
const BANK0_START: u8 = 0x25;

/// Select instructions for the whole module, producing PIC14 assembly text.
///
/// `addrs` maps global names to their bank-0 GPR addresses (from `alloc`).
/// SSA destinations are assigned fresh addresses per function: common RAM
/// (`0x70`→`0x7F`) first, then bank-0 GPRs from `0x25`.
pub fn select(m: &Module, addrs: &HashMap<String, u8>) -> String {
    let mut out = vec![
        "; pic8 -- integer spine milestone 1 (throwaway isel)".to_string(),
        "    list p=16f877a".to_string(),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    for f in &m.funcs {
        out.push(format!("{0}:", f.name));
        let mut slots: HashMap<String, u8> = HashMap::new();
        let mut next = COMMON_START;
        for b in &f.blocks {
            out.push(format!("{0}_L{1}:", f.name, b.label));
            for i in &b.insts {
                match i {
                    Inst::Load(l) => {
                        assert_eq!(l.ty, ir::Ty::I8, "isel: only i8 loads supported");
                        let (src, dst) = (
                            ptr_addr(&l.ptr, addrs, &slots),
                            slot(&mut slots, &mut next, &l.dst),
                        );
                        out.push(format!("    MOVF 0x{src:02X}, W"));
                        out.push(format!("    MOVWF 0x{dst:02X}"));
                    }
                    Inst::Store(s) => {
                        assert_eq!(s.ty, ir::Ty::I8, "isel: only i8 stores supported");
                        let dst = ptr_addr(&s.ptr, addrs, &slots);
                        match &s.val {
                            Val::Const(k) => {
                                assert!(
                                    *k >= 0 && *k <= 255,
                                    "isel: const {k} out of byte range"
                                );
                                out.push(format!("    MOVLW 0x{:02X}", *k as u8));
                                out.push(format!("    MOVWF 0x{dst:02X}"));
                            }
                            _ => {
                                let src = val_addr(&s.val, addrs, &slots);
                                out.push(format!("    MOVF 0x{src:02X}, W"));
                                out.push(format!("    MOVWF 0x{dst:02X}"));
                            }
                        }
                    }
                    Inst::Bin(b) => {
                        assert_eq!(b.ty, ir::Ty::I8, "isel: only i8 binops supported");
                        let da = slot(&mut slots, &mut next, &b.dst);
                        // Normalize commutative add: a const LHS is swapped to the
                        // RHS so the const-adder arm below is used, never reading a
                        // const as a file-register address. Both-const add folds at
                        // compile time; we don't fold yet.
                        let (a, b_op) = match (b.op, &b.a, &b.b) {
                            (BinOp::Add, Val::Const(_), Val::Const(_)) => {
                                panic!("isel: constant folding not implemented")
                            }
                            (BinOp::Add, Val::Const(_), _) => (&b.b, &b.a),
                            _ => (&b.a, &b.b),
                        };
                        match (b.op, b_op) {
                            (BinOp::Add, Val::Const(k)) => {
                                assert!(
                                    *k >= 0 && *k <= 255,
                                    "isel: const {k} out of byte range"
                                );
                                let aa = val_addr(a, addrs, &slots);
                                out.push(format!("    MOVF 0x{aa:02X}, W"));
                                out.push(format!("    ADDLW 0x{:02X}", *k as u8));
                                out.push(format!("    MOVWF 0x{da:02X}"));
                            }
                            (BinOp::Add, _) => {
                                let (aa, bb) = (
                                    val_addr(a, addrs, &slots),
                                    val_addr(b_op, addrs, &slots),
                                );
                                out.push(format!("    MOVF 0x{bb:02X}, W"));
                                out.push(format!("    ADDWF 0x{aa:02X}, W"));
                                out.push(format!("    MOVWF 0x{da:02X}"));
                            }
                            _ => panic!("isel: unsupported binop for milestone 1"),
                        }
                    }
                    Inst::Ret(None) => out.push("    RETURN".to_string()),
                    _ => panic!("isel: unsupported instruction for milestone 1"),
                }
            }
        }
    }
    out.push("__start:".to_string());
    out.push("    CALL main".to_string());
    out.push("    SLEEP".to_string());
    out.push("".to_string());
    out.push("    end".to_string());
    out.join("\n")
}

/// Assign (or return the existing) GPR address for an SSA destination `name`.
///
/// Common RAM `0x70`→`0x7F` is consumed first; once exhausted, bank-0 GPRs
/// from `0x25` are used. This is a milestone-1 approximation; overlay
/// allocation and spill handling land in a later milestone.
fn slot(slots: &mut HashMap<String, u8>, next: &mut u8, name: &str) -> u8 {
    if let Some(&a) = slots.get(name) {
        return a;
    }
    let a = *next;
    *next += 1;
    if *next > 0x80 {
        *next = BANK0_START;
    }
    slots.insert(name.to_string(), a);
    a
}

/// Resolve an operand value to a byte address.
///
/// `Val::Const` is only meaningful on an RHS operand; the general path here
/// is kept for completeness.
fn val_addr(v: &Val, addrs: &HashMap<String, u8>, slots: &HashMap<String, u8>) -> u8 {
    match v {
        Val::Reg(r) => *slots
            .get(r)
            .unwrap_or_else(|| panic!("isel: no slot for %{r}")),
        Val::Global(g) => *addrs
            .get(g)
            .unwrap_or_else(|| panic!("isel: no address for @{g}")),
        Val::Const(k) => {
            assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
            *k as u8
        }
    }
}

/// Resolve a memory pointer ("@name" global or "%name" slot) to an address.
fn ptr_addr(p: &str, addrs: &HashMap<String, u8>, slots: &HashMap<String, u8>) -> u8 {
    if let Some(g) = p.strip_prefix('@') {
        *addrs
            .get(g)
            .unwrap_or_else(|| panic!("isel: no address for @{g}"))
    } else {
        let name = p.trim_start_matches('%');
        *slots
            .get(name)
            .unwrap_or_else(|| panic!("isel: no slot for %{name}"))
    }
}
