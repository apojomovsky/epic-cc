//! Type-width validation boundary and runtime-call lowering for the PIC8
//! pipeline.
//!
//! `legalize` is where scalar ops that need runtime-library support leave the
//! IR's `Bin` form and become calls to injected routine functions:
//!
//! - `mul`/`udiv`/`urem`/`sdiv`/`srem` on i8/i16/i32 (the PIC16F877A has no
//!   hardware multiply/divide) become `Inst::Call` to the matching routine
//!   (`__mul_u8`/`__mul_u16`/`__mul_u32`, `__udiv_u8`/`__udiv_u16`/
//!   `__udiv_u32`, `__urem_u8`/`__urem_u16`/`__urem_u32`,
//!   `__sdiv_i8`/`__sdiv_i16`/`__sdiv_i32`, `__srem_i8`/`__srem_i16`/
//!   `__srem_i32`) with the dst/ty preserved and both operands copied as
//!   typed args.
//! - `shl`/`lshr`/`ashr` with a **const count stay as `Bin`** — isel inlines
//!   the fixed RLF/RRF sequence; with a **reg count** they become a call to
//!   the shift routine (`__shl_u8`/`__shl_u16`/`__shl_u32`,
//!   `__lshr_u8`/`__lshr_u16`/`__lshr_u32`,
//!   `__ashr_i8`/`__ashr_i16`/`__ashr_i32`), which masks the count and loops.
//! - `freeze` stays (isel lowers it as a byte copy).
//!
//! The used routine `Func`s are then injected into the module: ordinary
//! functions (name/ret/params per the ABI table below) with one empty block
//! holding only the scratch alloca, so `alloc` sizes the routine frame and
//! Tasks 3/4's recipe emitters read their working state from
//! `{func}::__scr` + offset. Only the routines actually used are injected
//! (cleaner text artifacts).

use ir::{Alloca, BinOp, Block, Call, CallArg, Func, Inst, Module, Param, Ty, Val};

pub fn legalize(m: Module) -> Module {
    let mut funcs = Vec::with_capacity(m.funcs.len() + 16);
    let mut used: Vec<String> = Vec::new();
    for f in m.funcs {
        let mut blocks = Vec::with_capacity(f.blocks.len());
        for b in f.blocks {
            let mut insts = Vec::with_capacity(b.insts.len());
            for inst in b.insts {
                match inst {
                    Inst::Bin(bin) => match lower_bin(&bin, &mut used) {
                        Some(call) => insts.push(call),
                        None => insts.push(Inst::Bin(bin)),
                    },
                    other => insts.push(other),
                }
            }
            blocks.push(Block { label: b.label, insts });
        }
        funcs.push(Func { name: f.name, ret: f.ret, params: f.params, blocks });
    }
    for name in &used {
        funcs.push(routine_func(name));
    }
    Module { globals: m.globals, funcs }
}

/// The runtime routine for a scalar binop, or `None` if legalize leaves the
/// op as a `Bin` (add/sub/and/or/xor, and i1 forms clang never emits).
fn routine_name(op: BinOp, ty: Ty) -> Option<&'static str> {
    match (op, ty) {
        (BinOp::Mul, Ty::I8) => Some("__mul_u8"),
        (BinOp::Mul, Ty::I16) => Some("__mul_u16"),
        (BinOp::Mul, Ty::I32) => Some("__mul_u32"),
        (BinOp::UDiv, Ty::I8) => Some("__udiv_u8"),
        (BinOp::UDiv, Ty::I16) => Some("__udiv_u16"),
        (BinOp::UDiv, Ty::I32) => Some("__udiv_u32"),
        (BinOp::URem, Ty::I8) => Some("__urem_u8"),
        (BinOp::URem, Ty::I16) => Some("__urem_u16"),
        (BinOp::URem, Ty::I32) => Some("__urem_u32"),
        (BinOp::SDiv, Ty::I8) => Some("__sdiv_i8"),
        (BinOp::SDiv, Ty::I16) => Some("__sdiv_i16"),
        (BinOp::SDiv, Ty::I32) => Some("__sdiv_i32"),
        (BinOp::SRem, Ty::I8) => Some("__srem_i8"),
        (BinOp::SRem, Ty::I16) => Some("__srem_i16"),
        (BinOp::SRem, Ty::I32) => Some("__srem_i32"),
        (BinOp::Shl, Ty::I8) => Some("__shl_u8"),
        (BinOp::Shl, Ty::I16) => Some("__shl_u16"),
        (BinOp::Shl, Ty::I32) => Some("__shl_u32"),
        (BinOp::LShr, Ty::I8) => Some("__lshr_u8"),
        (BinOp::LShr, Ty::I16) => Some("__lshr_u16"),
        (BinOp::LShr, Ty::I32) => Some("__lshr_u32"),
        (BinOp::AShr, Ty::I8) => Some("__ashr_i8"),
        (BinOp::AShr, Ty::I16) => Some("__ashr_i16"),
        (BinOp::AShr, Ty::I32) => Some("__ashr_i32"),
        _ => None,
    }
}

/// Rewrite one `Inst::Bin` into the runtime call, recording the routine as
/// used. Returns `None` when the binop stays as-is: non-lowered ops, and
/// const-count shifts (isel inlines those — the count arrives as a `Const`).
fn lower_bin(b: &ir::Bin, used: &mut Vec<String>) -> Option<Inst> {
    if matches!(b.op, BinOp::Shl | BinOp::LShr | BinOp::AShr) {
        if matches!(b.b, Val::Const(_)) {
            return None;
        }
    }
    let func = routine_name(b.op, b.ty)?;
    if !used.iter().any(|u| u == func) {
        used.push(func.to_string());
    }
    Some(Inst::Call(Call {
        dst: Some(b.dst.clone()),
        ty: Some(b.ty),
        func: func.to_string(),
        args: vec![
            CallArg { ty: Some(b.ty), val: b.a.clone(), byval: None, sret: false },
            CallArg { ty: Some(b.ty), val: b.b.clone(), byval: None, sret: false },
        ],
    }))
}

fn param(name: &str, width: u8) -> Param {
    Param { name: name.into(), width, byval: None, sret: false }
}

/// The injected runtime routine definitions. Each is an ordinary function
/// with one empty block containing only the scratch alloca, so `alloc`
/// places the frame and Tasks 3/4's recipe emitters can resolve every slot
/// address from the map (`{func}::{param}`, `{func}::__scr`).
///
/// # The scratch layout contract (sizes + offsets)
///
/// These byte offsets are the cross-task contract: Task 2 injects the
/// buffers, Task 3 emits the mul/div/rem recipe bodies against them, Task 4
/// the shift recipe bodies. The recipes read their inputs from the param
/// slots (`a`/`b`, `num`/`den`, `val`/`cnt`), write the result to the retval
/// slots, and use `__scr` strictly by offset. All addresses must stay in
/// bank 0 (≤ 0xFF) — the recipes' loops are skip-sensitive, so no BANKSEL
/// may be inserted between a test and its target.
///
/// | routine | `__scr` size | offsets |
/// |---|---|---|
/// | `__mul_u8` | 6 | `bk`@0 (multiplier backup, shifted to test bits), `cnt`@1 (loop counter, 8), `r_lo`@2 / `r_hi`@3 (16-bit running product), `t_lo`@4 / `t_hi`@5 (shifted multiplicand) |
/// | `__mul_u16` | 14 | `bk_lo`@0 / `bk_hi`@1 (multiplier backup), `cnt`@2 (loop counter, 16), `r`@3-6 (32-bit running product), `t`@7-10 (shifted multiplicand), `spare`@11-13 (recipe scratch) |
/// | `__udiv_u8`, `__urem_u8` | 4 | `rem_lo`@0 / `rem_hi`@1 (partial remainder — 2 bytes: the 8-bit rem shift can carry), `cnt`@2 (loop counter, 8), `restore`@3 (restore-step scratch) |
/// | `__udiv_u16`, `__urem_u16` | 7 | `rem`@0-1 (partial remainder), `cnt`@2 (loop counter, 16), `spare`@3 (recipe scratch), `restore`@4-6 (restore-step scratch) |
/// | `__sdiv_i8`, `__srem_i8` | 5 | `flags`@0 (sign state: bit0 = negate quotient, bit1 = negate remainder; `\|num\|`/`\|den\|` live in the param slots), `rem_lo`@1 / `rem_hi`@2, `cnt`@3, `restore`@4 |
/// | `__sdiv_i16`, `__srem_i16` | 7 | `flags`@0 (as i8), `rem`@1-2, `cnt`@3, `restore`@4-5, `spare`@6 |
/// | `__shl_u8`, `__lshr_u8`, `__ashr_i8` | 3 | `cnt`@0 (masked count / loop counter — the value shifts in the `val` param slot), `spare`@1-2 (recipe scratch) |
/// | `__shl_u16`, `__lshr_u16`, `__ashr_i16` | 4 | `cnt`@0-1 (masked count / loop counter), `spare`@2-3 (recipe scratch) |
/// | `__mul_u32` | 11 | `bk_lo`@0 / `bk_hi`@1 (multiplier backup — 2 bytes: the low 16 bits first, reloaded from `b`'s high half for the second 16 of the 32 iterations), `cnt`@2 (loop counter, 32), `r`@3-6 (32-bit running product — the low 32 bits of the full product), `t`@7-10 (shifted multiplicand — 4 bytes, shifting left with wraparound: the shifted-out high bits are DISCARDED, i32 `mul` wraps) |
/// | `__udiv_u32`, `__urem_u32` | 10 | `rem`@0-3 (partial remainder — full 32 bits, never carries out for a 32/32 divide), `den`@4-7 (denominator copy — the divmod subtracts/restores against this, so the param slot is untouched), `cnt`@8 (loop counter, 32), `spare`@9 (recipe scratch) |
/// | `__sdiv_i32`, `__srem_i32` | 12 | the divmod part at the unsigned offsets — `rem`@0-3, `den`@4-7, `cnt`@8, `spare`@9 — plus `flags`@10 (sign state: bit0 = negate quotient = num<0 XOR den<0, bit1 = negate remainder = num<0), `spare`@11 |
/// | `__shl_u32`, `__lshr_u32`, `__ashr_i32` | 2 | `cnt`@0 (masked count / loop counter — the value shifts in the `val` param slot), `spare`@1 (recipe scratch) |
///
/// Notes: div-by-zero is LLVM poison — the loop runs (den = 0 ⇒ quotient
/// 0xFFFF, remainder 0), any value is legal, no guard. Variable-shift counts
/// arrive unmasked and are masked to `width - 1` inside the routine. The
/// signed wrappers abs in place in the param slots (unsigned abs, so INT_MIN
/// is safe), run the unsigned divmod, then negate per the flags byte.
fn routine_func(name: &str) -> Func {
    let (ret, params, scr) = match name {
        "__mul_u8" => (Ty::I8, vec![param("a", 1), param("b", 1)], 6),
        "__mul_u16" => (Ty::I16, vec![param("a", 2), param("b", 2)], 14),
        "__mul_u32" => (Ty::I32, vec![param("a", 4), param("b", 4)], 11),
        "__udiv_u8" | "__urem_u8" => (Ty::I8, vec![param("num", 1), param("den", 1)], 4),
        "__udiv_u16" | "__urem_u16" => (Ty::I16, vec![param("num", 2), param("den", 2)], 7),
        "__udiv_u32" | "__urem_u32" => (Ty::I32, vec![param("num", 4), param("den", 4)], 10),
        "__sdiv_i8" | "__srem_i8" => (Ty::I8, vec![param("num", 1), param("den", 1)], 5),
        "__sdiv_i16" | "__srem_i16" => (Ty::I16, vec![param("num", 2), param("den", 2)], 7),
        "__sdiv_i32" | "__srem_i32" => (Ty::I32, vec![param("num", 4), param("den", 4)], 12),
        "__shl_u8" | "__lshr_u8" | "__ashr_i8" => (Ty::I8, vec![param("val", 1), param("cnt", 1)], 3),
        "__shl_u16" | "__lshr_u16" | "__ashr_i16" => (Ty::I16, vec![param("val", 2), param("cnt", 2)], 4),
        "__shl_u32" | "__lshr_u32" | "__ashr_i32" => (Ty::I32, vec![param("val", 4), param("cnt", 4)], 2),
        other => panic!("legalize: unknown runtime routine {other}"),
    };
    Func {
        name: name.into(),
        ret: Some(ret),
        params,
        blocks: vec![Block {
            label: "entry".into(),
            insts: vec![Inst::Alloca(Alloca { dst: "__scr".into(), size: scr })],
        }],
    }
}
