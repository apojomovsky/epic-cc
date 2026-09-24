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
//! `shl`/`lshr`/`ashr` with a const count stay as `Bin`: isel inlines
//! the fixed RLF/RRF sequence. With a reg count they become a call to
//! the shift routine (`__shl_u8`/`__shl_u16`/`__shl_u32`,
//! `__lshr_u8`/`__lshr_u16`/`__lshr_u32`,
//! `__ashr_i8`/`__ashr_i16`/`__ashr_i32`), which masks the count and loops.
//! - `freeze` stays (isel lowers it as a byte copy).
//! The soft-float lowering turns every f32 op into a runtime call:
//! `fadd`/`fsub`/`fmul`/`fdiv` become `__add_f32`/`__sub_f32`/`__mul_f32`/
//! `__div_f32` (dst/ty preserved, both operands copied as float args);
//! `fcmp <pred>` becomes `%c = call i8 @__cmp_f32(a, b)` plus the per-predicate
//! icmp/select materialization tree over the tri-state byte
//! (0 = equal, 1 = a < b, 2 = a > b, 3 = unordered). An OR predicate
//! becomes `select i1 <c==k1>, i1 true, i1 <c==k2>`, with no i1 binop (isel
//! rejects those); `fptosi`/`fptoui`/`sitofp`/`uitofp` become the four
//! conversion routines; `fpext`/`fptrunc` (f32 to f32, since double equals
//! float on msp430) become a plain `freeze` copy, no call.
//!
//! The used routine `Func`s are then injected into the module: ordinary
//! functions (name/ret/params per the ABI table below) with one empty block
//! holding only the scratch alloca, so `alloc` sizes the routine frame and
//! the recipe emitters read their working state from
//! `{func}::__scr` + offset. Only the routines actually used are injected
//! (cleaner text artifacts).
//!
//! A routine both the main and the interrupt context reach is injected
//! twice (`__mul_u8` and `__mul_u8_isr`), so the two contexts share
//! no frame. Without the split, an interrupt taken partway through main's
//! multiply re-enters the same scratch bytes and main resumes against the
//! ISR's state, with no diagnostic. See `split_isr_routines`.

use std::collections::{HashMap, HashSet};

use ir::{
    Alloca, Bin, BinOp, Block, Call, CallArg, FBinOp, FloatConvOp, Func, Gep, GepBase, Icmp, Inst,
    IntToPtr, MemLen, Module, Param, Select, Sext, Trunc, Ty, Val, Zext,
};

pub fn legalize(m: Module) -> Module {
    // Interrupt duplication happens in two layers. User functions split
    // here, before the lowering loop, because their calls already exist.
    // The runtime routines split after it (`split_isr_routines`), because
    // the loop is what creates their calls. The returned sets record the
    // originals whose stored address was rewritten to a copy, so the
    // candidate filler can accept the copy at every other context's site
    // (they all read the same shared storage, epic-cc#568).
    let (m, stored_lo, stored_hi, spellings) = duplicate_isr_shared(m);
    let m = sink_ptr_select_funcs(m);
    let m = narrow_div_rem_tails(m);
    let mut funcs = Vec::with_capacity(m.funcs.len() + 16);
    let mut used: Vec<String> = Vec::new();
    // Fresh SSA names for the fcmp materialization intermediates (the call
    // dst and the icmp temps), seeded with every name the module defines so
    // each tree avoids collision with a user reg.
    let mut names = FreshNames::from_module(&m);
    for f in m.funcs {
        let mut blocks = Vec::with_capacity(f.blocks.len());
        for b in f.blocks {
            let mut insts = Vec::with_capacity(b.insts.len());
            for inst in b.insts {
                match inst {
                    Inst::Bin(bin) => {
                        // i64 arithmetic is a documented limitation: only
                        // the aggregate load/store copy (the HAL handle
                        // shape) is supported. A Bin on i64
                        // panics rather than silently miscompiling (epic-cc#125).
                        assert!(
                            bin.ty != Ty::I64,
                            "legalize: i64 arithmetic not supported (only i64 aggregate copies, epic-cc#125)"
                        );
                        match fold_const_bin(&bin).or_else(|| lower_bin(&bin, &mut used)) {
                            Some(folded_or_call) => insts.push(folded_or_call),
                            None => insts.push(Inst::Bin(bin)),
                        }
                    }
                    Inst::Icmp(icmp) => match fold_const_icmp(&icmp) {
                        Some(frozen) => insts.push(frozen),
                        None => insts.push(Inst::Icmp(icmp)),
                    },
                    Inst::FloatBin(fb) => insts.push(lower_fbin(&fb, &mut used)),
                    Inst::Fcmp(fc) => insts.extend(lower_fcmp(&fc, &mut used, &mut names)),
                    Inst::FloatConv(fc) => insts.push(lower_fconv(&fc, &mut used)),
                    Inst::Call(c) => {
                        if c.func.starts_with("llvm.") {
                            // A clang-emitted intrinsic (`llvm.smax.*`,
                            // `llvm.smin.*`, `llvm.abs.*`) becomes the
                            // icmp/select tree; an unknown intrinsic panics
                            // so a new one surfaces as a clear error
                            // instead of a hole the assembler misses.
                            insts.extend(lower_intrinsic(&c, &mut names, &mut used));
                        } else {
                            insts.push(Inst::Call(c));
                        }
                    }
                    other => insts.push(other),
                }
            }
            blocks.push(Block {
                label: b.label,
                insts,
            });
        }
        funcs.push(Func {
            name: f.name,
            ret: f.ret,
            params: f.params,
            blocks,
            isr: f.isr,
            irq_priority: f.irq_priority,
            naked: f.naked,
            variadic: f.variadic,
        });
    }
    // Runtime-routine duplication for the interrupt context. The user-level
    // duplication ran before the loop above, but the routine CALLs are
    // created BY that loop, so the routines can only be split here. A
    // routine both contexts reach gets an `_isr` copy with its own frame;
    // without it, an ISR that preempts main inside `__mul_u8` re-enters the
    // one shared frame and clobbers main's in-flight state.
    let isr_used = split_isr_routines(&mut funcs, &used);
    for name in &used {
        funcs.push(routine_func(name));
    }
    for name in &isr_used {
        let base = name
            .strip_suffix("_isr_high")
            .or_else(|| name.strip_suffix("_isr"))
            .expect("legalize: isr routine name must end in _isr or _isr_high");
        let mut f = routine_func(base);
        f.name = name.clone();
        funcs.push(f);
    }
    let mut m = Module {
        globals: m.globals,
        funcs,
        module_asm: m.module_asm,
    };
    fill_indirect_callees(&mut m, &stored_lo, &stored_hi, &spellings);
    m
}

/// A pointer-returning function whose body is a pure chain of cond/gep
/// defs ending in a pointer select cannot lower through a CALL boundary:
/// iselcore folds pointer SELECTs and GEPs inside the caller, but a pointer
/// value returned from a call has no foldable base in the caller (the
/// callee's registers are private). Sinks such a function into its call
/// sites: each `%r = call @f(args)` becomes a cloned copy of the body's
/// value-producing instructions with params substituted by args. A
/// non-sinkable body panics in iselcore: no lowering path exists there.
fn sink_ptr_select_funcs(m: Module) -> Module {
    let mut sinkable: HashMap<String, Vec<Inst>> = HashMap::new();
    for f in &m.funcs {
        if f.ret != Some(Ty::I16) || f.isr || f.naked || f.blocks.len() != 1 {
            continue;
        }
        let insts = &f.blocks[0].insts;
        let (Some(Inst::Ret(Some((Ty::I16, Val::Reg(ret_reg))), _)), rest) =
            (insts.last(), &insts[..insts.len().saturating_sub(1)])
        else {
            continue;
        };
        // The final def is either a pointer select over constant arms or a
        // plain GEP (the `return &addrs[inst]` shape, whose runtime term the
        // caller must carry). Both are pointer VALUES the caller can fold.
        let final_dst = match rest.last() {
            Some(Inst::Select(s)) if s.ptr && s.dst == *ret_reg => Some(s.dst.clone()),
            Some(Inst::Gep(g)) if g.dst == *ret_reg => Some(g.dst.clone()),
            _ => None,
        };
        let Some(_) = final_dst else {
            continue;
        };
        let body = &rest[..rest.len()];
        let defined: HashSet<&str> = body.iter().filter_map(inst_dst).collect();
        let mut ok = true;
        for inst in body {
            for reg in inst_regs(inst) {
                if !f.params.iter().any(|p| p.name == reg) && !defined.contains(reg.as_str()) {
                    ok = false;
                }
            }
        }
        if ok {
            sinkable.insert(f.name.clone(), body.to_vec());
        }
    }
    if sinkable.is_empty() {
        return m;
    }
    // Param-name lists of the sinkable funcs, captured before `m.funcs` is
    // consumed by the caller rewrite loop below.
    let sink_params: HashMap<String, Vec<String>> = m
        .funcs
        .iter()
        .filter(|f| sinkable.contains_key(&f.name))
        .map(|f| {
            (
                f.name.clone(),
                f.params.iter().map(|p| p.name.clone()).collect(),
            )
        })
        .collect();
    let mut funcs = Vec::with_capacity(m.funcs.len());
    for mut f in m.funcs {
        let mut used_names: HashSet<String> = HashSet::new();
        for p in &f.params {
            used_names.insert(p.name.clone());
        }
        for b in &f.blocks {
            for inst in &b.insts {
                if let Some(d) = inst_dst(inst) {
                    used_names.insert(d.to_string());
                }
            }
        }
        let mut fresh = FreshNames {
            used: used_names,
            next: 0,
        };
        let mut blocks = Vec::with_capacity(f.blocks.len());
        for b in f.blocks {
            let mut insts = Vec::with_capacity(b.insts.len());
            for inst in b.insts {
                if let Inst::Call(c) = &inst {
                    if let Some(body) = sinkable.get(&c.func) {
                        if c.dst.is_some() && c.ty == Some(Ty::I16) {
                            let mut subst: HashMap<String, Val> = HashMap::new();
                            if let Some(pnames) = sink_params.get(&c.func) {
                                for (i, p) in pnames.iter().enumerate() {
                                    if let Some(a) = c.args.get(i) {
                                        subst.insert(p.clone(), a.val.clone());
                                    }
                                }
                            }
                            let call_dst = c.dst.clone().unwrap();
                            // Every body dst gets a fresh name (params were
                            // substituted above), so clones at different call
                            // sites stay distinct; the final select's dst
                            // becomes the call's dst.
                            let mut rename_map: HashMap<String, String> = HashMap::new();
                            for binst in body {
                                if let Some(d) = inst_dst(binst) {
                                    rename_map.insert(d.to_string(), fresh.fresh());
                                }
                            }
                            let mut cloned = Vec::with_capacity(body.len());
                            for binst in body {
                                cloned.push(substitute_regs(binst, &subst, &rename_map));
                            }
                            // The final def (select or gep) becomes the call's
                            // dst.
                            match cloned.last_mut() {
                                Some(Inst::Select(s)) => s.dst = call_dst,
                                Some(Inst::Gep(g)) => g.dst = call_dst,
                                _ => {}
                            }
                            insts.extend(cloned);
                            continue;
                        }
                    }
                }
                insts.push(inst);
            }
            blocks.push(Block {
                label: b.label,
                insts,
            });
        }
        f.blocks = blocks;
        funcs.push(f);
    }
    let calls: HashSet<String> = funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            Inst::Call(c) => Some(c.func.clone()),
            _ => None,
        })
        .collect();
    funcs.retain(|f| !sinkable.contains_key(&f.name) || calls.contains(&f.name));
    Module {
        globals: m.globals,
        funcs,
        module_asm: m.module_asm,
    }
}

/// Rewrite the decimal div-rem expansion so its remainder is computed at
/// the width it is actually observed at (epic-cc#622).
///
/// clang emits `v % 10` as `q = udiv i16 v, 10`, `m = mul i16 q, 246`
/// (246 is the low byte of -10), `s = add i16 m, v`, `r = trunc i16 s to
/// i8`. Only `r`'s low byte is ever used. Modular arithmetic lets the
/// whole tail run at i8: `(m + v) mod 256 == ((q mod 256)*246 + (v mod
/// 256)) mod 256`, because a product and a sum only depend on their
/// operands' low bytes modulo 256. Narrowing `mul i16` to `mul i8` lets
/// the existing 4-word `__mul_u8` replace the 19-word `__mul_u16`, and
/// drops the 2-byte argument staging to 1 byte per operand.
///
/// The rewrite fires only when the shape is exact and single-use: the
/// `add`'s sole consumer is the `trunc`, and the `mul`'s sole consumer is
/// the `add`. A second use of either keeps the i16 form, which is always
/// correct.
fn narrow_div_rem_tails(m: Module) -> Module {
    let mut fresh = FreshNames::from_module(&m);
    let mut funcs = Vec::with_capacity(m.funcs.len());
    for f in m.funcs {
        // dst -> defining inst, for the operand lookup.
        let defs: HashMap<String, Inst> = f
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter_map(|i| inst_dst(i).map(|d| (d.to_string(), i.clone())))
            .collect();
        // Uses of each reg, so a shape is rewritten only when it is the
        // sole consumer chain.
        let mut uses: HashMap<String, usize> = HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                for r in inst_reads(inst) {
                    *uses.entry(r).or_insert(0) += 1;
                }
            }
        }
        // For each qualifying shape: the `mul`'s dst takes the narrowed
        // multiply, the `add`'s dst takes the narrowed sum (under the
        // trunc's own dst), and the trunc itself disappears.
        let mut replace: HashMap<String, Vec<Inst>> = HashMap::new();
        let mut drop: HashSet<String> = HashSet::new();
        for b in &f.blocks {
            for inst in &b.insts {
                let Inst::Trunc(t) = inst else { continue };
                if t.to != Ty::I8 {
                    continue;
                }
                // Only i16/i32 tails: those are the routine-backed
                // multiplies (`__mul_u16`/`__mul_u32`) this shortens.
                if !matches!(t.from, Ty::I16 | Ty::I32) {
                    continue;
                }
                let w = t.from;
                let Val::Reg(sum) = &t.val else { continue };
                if uses.get(sum) != Some(&1) {
                    continue;
                }
                let Some(Inst::Bin(add)) = defs.get(sum) else {
                    continue;
                };
                if add.op != BinOp::Add || add.ty != w {
                    continue;
                }
                // One arm is the `mul w q, K`; the other is `v`.
                let (mul_dst, other) = match (&add.a, &add.b) {
                    (Val::Reg(r), o) if defs.get(r).is_some_and(|d| is_mul_const(d, w)) => {
                        (r.clone(), o.clone())
                    }
                    (o, Val::Reg(r)) if defs.get(r).is_some_and(|d| is_mul_const(d, w)) => {
                        (r.clone(), o.clone())
                    }
                    _ => continue,
                };
                // The narrowed tail needs the addend to be a register: the
                // rewrite emits `trunc <w> <addend> to i8`, and isel rejects
                // a const source outright (`const source Trunc not yet
                // supported`) while a global would be read as an address.
                // The clang shape is always `add w %m, %v` with `%v` a
                // loaded register, so the guard costs nothing real.
                if !matches!(other, Val::Reg(_)) {
                    continue;
                }
                if uses.get(&mul_dst) != Some(&1) {
                    continue;
                }
                let Some(Inst::Bin(mul)) = defs.get(&mul_dst) else {
                    continue;
                };
                let (Val::Reg(qreg), Val::Const(k)) = (&mul.a, &mul.b) else {
                    continue;
                };
                // `q`'s own use count is irrelevant: it keeps its width
                // and every other use reads the full value. Only the
                // multiply and the sum must be single-use, because those
                // are the results this rewrite changes the width of.
                // The narrowed tail stands where the multiply did: the low
                // byte of `q`, the 8x8 product, the low byte of `v`, then
                // the 8-bit sum carrying the trunc's own name.
                let q8 = fresh.fresh();
                let p8 = fresh.fresh();
                let v8 = fresh.fresh();
                replace.insert(
                    mul_dst.clone(),
                    vec![
                        Inst::Trunc(Trunc {
                            dst: q8.clone(),
                            from: w,
                            val: Val::Reg(qreg.clone()),
                            to: Ty::I8,
                            loc: mul.loc.clone(),
                        }),
                        Inst::Bin(Bin {
                            dst: p8.clone(),
                            op: BinOp::Mul,
                            ty: Ty::I8,
                            a: Val::Reg(q8),
                            b: Val::Const(*k),
                            loc: mul.loc.clone(),
                        }),
                    ],
                );
                replace.insert(
                    sum.clone(),
                    vec![
                        Inst::Trunc(Trunc {
                            dst: v8.clone(),
                            from: w,
                            val: other,
                            to: Ty::I8,
                            loc: add.loc.clone(),
                        }),
                        Inst::Bin(Bin {
                            dst: t.dst.clone(),
                            op: BinOp::Add,
                            ty: Ty::I8,
                            a: Val::Reg(p8),
                            b: Val::Reg(v8),
                            loc: t.loc.clone(),
                        }),
                    ],
                );
                drop.insert(t.dst.clone());
            }
        }
        if replace.is_empty() {
            funcs.push(f);
            continue;
        }
        let mut blocks = Vec::with_capacity(f.blocks.len());
        for b in f.blocks {
            let mut insts: Vec<Inst> = Vec::with_capacity(b.insts.len());
            for inst in b.insts {
                if let Some(d) = inst_dst(&inst) {
                    if let Some(repl) = replace.get(d) {
                        insts.extend(repl.iter().cloned());
                        continue;
                    }
                    if drop.contains(d) {
                        continue;
                    }
                }
                insts.push(inst);
            }
            blocks.push(Block {
                label: b.label,
                insts,
            });
        }
        funcs.push(Func {
            name: f.name,
            ret: f.ret,
            params: f.params,
            blocks,
            isr: f.isr,
            irq_priority: f.irq_priority,
            naked: f.naked,
            variadic: f.variadic,
        });
    }
    Module {
        globals: m.globals,
        funcs,
        module_asm: m.module_asm,
    }
}

/// Whether `inst` is `mul <ty> <reg>, <const>` at `ty`.
fn is_mul_const(inst: &Inst, ty: Ty) -> bool {
    matches!(
        inst,
        Inst::Bin(Bin {
            op: BinOp::Mul,
            ty: t,
            b: Val::Const(_),
            ..
        }) if *t == ty
    )
}

/// The SSA registers an instruction reads, as the pointer-sinking pass
/// has always enumerated them. Deliberately left as it was: making this
/// enumeration stricter would turn some currently-sinkable functions into
/// non-sinkable ones, and a non-sinkable body has no iselcore lowering
/// (it panics). The narrowing pass uses `inst_reads`, which is complete.
fn inst_regs(inst: &Inst) -> Vec<String> {
    let mut regs = Vec::new();
    let mut push = |v: &Val| {
        if let Val::Reg(r) = v {
            regs.push(r.clone());
        }
    };
    match inst {
        Inst::Bin(b) => {
            push(&b.a);
            push(&b.b);
        }
        Inst::Icmp(c) => {
            push(&c.a);
            push(&c.b);
        }
        Inst::Select(s) => {
            push(&s.a);
            push(&s.b);
        }
        Inst::Zext(z) => push(&z.val),
        Inst::Sext(x) => push(&x.val),
        Inst::Trunc(t) => push(&t.val),
        Inst::IntToPtr(p) => push(&p.val),
        Inst::Gep(g) => {
            if let GepBase::Reg(r) = &g.base {
                regs.push(r.clone());
            }
            for (_, t) in &g.terms {
                regs.push(t.clone());
            }
        }
        _ => {}
    }
    regs
}

/// Every SSA register an instruction reads, across all operand shapes.
///
/// The narrowing pass is only sound when the widened tail's low byte is
/// the whole observable result, so a use it fails to see would drop a
/// live 16-bit value: the enumeration must cover every variant, not just
/// the value operands (`Store`'s value, `Ret`'s operand, `Phi`'s incoming
/// arms, `Switch`'s scrutinee, `Memcpy`'s operands). Kept separate from
/// `inst_regs` so a caller with its own established semantics is not
/// perturbed by this one's completeness (epic-cc#622).
fn inst_reads(inst: &Inst) -> Vec<String> {
    fn push(v: &Val, regs: &mut Vec<String>) {
        if let Val::Reg(r) = v {
            regs.push(r.clone());
        }
    }
    let mut regs = Vec::new();
    match inst {
        Inst::Load(l) => regs.push(l.ptr.strip_prefix('%').unwrap_or(&l.ptr).to_string()),
        Inst::Store(s) => {
            regs.push(s.ptr.strip_prefix('%').unwrap_or(&s.ptr).to_string());
            push(&s.val, &mut regs);
        }
        Inst::Bin(b) => {
            push(&b.a, &mut regs);
            push(&b.b, &mut regs);
        }
        Inst::Ret(Some((_, v)), _) => push(v, &mut regs),
        Inst::Ret(None, _) => {}
        Inst::Zext(z) => push(&z.val, &mut regs),
        Inst::Sext(x) => push(&x.val, &mut regs),
        Inst::Trunc(t) => push(&t.val, &mut regs),
        Inst::IntToPtr(p) => push(&p.val, &mut regs),
        Inst::Icmp(c) => {
            push(&c.a, &mut regs);
            push(&c.b, &mut regs);
        }
        Inst::Select(s) => {
            push(&s.cond, &mut regs);
            push(&s.a, &mut regs);
            push(&s.b, &mut regs);
        }
        Inst::Call(c) => {
            for a in &c.args {
                push(&a.val, &mut regs);
            }
            if !c.callees.is_empty() {
                regs.push(c.func.clone());
            }
        }
        Inst::Br(_) => {}
        Inst::BrCond(b) => push(&b.cond, &mut regs),
        Inst::Switch(s) => push(&s.val, &mut regs),
        Inst::Phi(p) => {
            for (v, _) in &p.incoming {
                push(v, &mut regs);
            }
        }
        Inst::Gep(g) => {
            if let GepBase::Reg(r) = &g.base {
                regs.push(r.clone());
            }
            for (_, t) in &g.terms {
                regs.push(t.clone());
            }
        }
        Inst::Alloca(_) => {}
        Inst::Memcpy(mc) => {
            push(&mc.dst, &mut regs);
            push(&mc.src, &mut regs);
            if let MemLen::Reg(v) = &mc.len {
                push(v, &mut regs);
            }
        }
        Inst::Freeze(f) => push(&f.val, &mut regs),
        Inst::VaArg(v) => regs.push(v.ptr.clone()),
        Inst::VaStart(v) => regs.push(v.list.clone()),
        Inst::FloatBin(b) => {
            push(&b.a, &mut regs);
            push(&b.b, &mut regs);
        }
        Inst::Fcmp(c) => {
            push(&c.a, &mut regs);
            push(&c.b, &mut regs);
        }
        Inst::FloatConv(c) => push(&c.val, &mut regs),
        Inst::Asm(a) => {
            for op in &a.operands {
                regs.push(op.ptr.strip_prefix('%').unwrap_or(&op.ptr).to_string());
            }
        }
    }
    regs
}

fn substitute_regs(
    inst: &Inst,
    subst: &HashMap<String, Val>,
    rename: &HashMap<String, String>,
) -> Inst {
    let sub = |v: &Val| -> Val {
        match v {
            Val::Reg(r) => {
                if let Some(n) = rename.get(r) {
                    Val::Reg(n.clone())
                } else {
                    subst.get(r).cloned().unwrap_or_else(|| Val::Reg(r.clone()))
                }
            }
            _ => v.clone(),
        }
    };
    match inst {
        Inst::Bin(b) => Inst::Bin(Bin {
            dst: rename.get(&b.dst).cloned().unwrap_or_else(|| b.dst.clone()),
            op: b.op,
            ty: b.ty,
            a: sub(&b.a),
            b: sub(&b.b),
            loc: b.loc.clone(),
        }),
        Inst::Icmp(c) => Inst::Icmp(Icmp {
            dst: rename.get(&c.dst).cloned().unwrap_or_else(|| c.dst.clone()),
            pred: c.pred.clone(),
            ty: c.ty,
            a: sub(&c.a),
            b: sub(&c.b),
            loc: c.loc.clone(),
        }),
        Inst::Select(s) => Inst::Select(Select {
            dst: rename.get(&s.dst).cloned().unwrap_or_else(|| s.dst.clone()),
            cond: sub(&s.cond),
            ty: s.ty,
            a: sub(&s.a),
            b: sub(&s.b),
            ptr: s.ptr,
            loc: s.loc.clone(),
        }),
        Inst::Zext(z) => Inst::Zext(Zext {
            dst: rename.get(&z.dst).cloned().unwrap_or_else(|| z.dst.clone()),
            from: z.from,
            val: sub(&z.val),
            to: z.to,
            loc: z.loc.clone(),
        }),
        Inst::Sext(x) => Inst::Sext(Sext {
            dst: rename.get(&x.dst).cloned().unwrap_or_else(|| x.dst.clone()),
            from: x.from,
            val: sub(&x.val),
            to: x.to,
            loc: x.loc.clone(),
        }),
        Inst::Trunc(t) => Inst::Trunc(Trunc {
            dst: rename.get(&t.dst).cloned().unwrap_or_else(|| t.dst.clone()),
            from: t.from,
            val: sub(&t.val),
            to: t.to,
            loc: t.loc.clone(),
        }),
        Inst::IntToPtr(p) => Inst::IntToPtr(IntToPtr {
            dst: rename.get(&p.dst).cloned().unwrap_or_else(|| p.dst.clone()),
            from: p.from,
            val: sub(&p.val),
            to: p.to,
            loc: p.loc.clone(),
        }),
        Inst::Gep(g) => {
            let base = match &g.base {
                GepBase::Global(n) => GepBase::Global(n.clone()),
                GepBase::Reg(r) => match subst.get(r) {
                    Some(Val::Reg(nr)) => GepBase::Reg(nr.clone()),
                    Some(Val::Global(ng)) => GepBase::Global(ng.clone()),
                    _ => GepBase::Reg(rename.get(r).cloned().unwrap_or_else(|| r.clone())),
                },
            };
            let terms = g
                .terms
                .iter()
                .map(|(sc, r)| match subst.get(r) {
                    Some(Val::Reg(nr)) => (*sc, nr.clone()),
                    Some(Val::Global(ng)) => (*sc, ng.clone()),
                    _ => (*sc, rename.get(r).cloned().unwrap_or_else(|| r.clone())),
                })
                .collect();
            Inst::Gep(Gep {
                dst: rename.get(&g.dst).cloned().unwrap_or_else(|| g.dst.clone()),
                base,
                k: g.k,
                terms,
                loc: g.loc.clone(),
            })
        }
        other => other.clone(),
    }
}

/// Split the runtime routines that a second live context reaches: each
/// gets a suffixed copy per reaching priority (`_isr` for low,
/// `_isr_high` for high) and every routine call inside that priority's
/// context is rewritten to it. Returns the suffixed names to inject, in
/// `used` order so the emitted module text stays deterministic.
///
/// A routine only one context reaches is left shared: there is no
/// second-context caller whose frame it could clobber, and a second
/// copy would spend flash and RAM for nothing. This mirrors
/// `duplicate_isr_shared`'s policy for user functions, one layer down.
fn split_isr_routines(funcs: &mut [Func], used: &[String]) -> Vec<String> {
    if used.is_empty() || !funcs.iter().any(|f| f.isr) {
        return Vec::new();
    }
    // Caller -> callee edges over the POST-lowering module, so the routine
    // calls the loop above just created are visible.
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for f in funcs.iter() {
        let edges = adj.entry(f.name.clone()).or_default();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    edges.push(c.func.clone());
                }
            }
        }
    }
    let lo_roots: Vec<&str> = funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority != 1)
        .map(|f| f.name.as_str())
        .collect();
    let hi_roots: Vec<&str> = funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority == 1)
        .map(|f| f.name.as_str())
        .collect();
    let lo_ctx = reachable(&lo_roots, &adj);
    let hi_ctx = reachable(&hi_roots, &adj);
    let main_ctx = reachable(&["main"], &adj);
    // A routine needs a priority's copy when that priority can run it
    // while another context's frame for it is live: reachable from the
    // priority and from anywhere else live at the same time.
    let shared_lo: HashSet<&str> = used
        .iter()
        .map(String::as_str)
        .filter(|r| lo_ctx.contains(*r) && (main_ctx.contains(*r) || hi_ctx.contains(*r)))
        .collect();
    let shared_hi: HashSet<&str> = used
        .iter()
        .map(String::as_str)
        .filter(|r| hi_ctx.contains(*r) && (main_ctx.contains(*r) || lo_ctx.contains(*r)))
        .collect();
    if shared_lo.is_empty() && shared_hi.is_empty() {
        return Vec::new();
    }
    // Rewrite the shared routines' calls inside each priority's context
    // only. The main-context callers (including a shared user function's
    // ORIGINAL, which duplicate_isr_shared left main-only) keep the base
    // name.
    for f in funcs.iter_mut() {
        let in_lo = lo_ctx.contains(&f.name);
        let in_hi = hi_ctx.contains(&f.name);
        if !in_lo && !in_hi {
            continue;
        }
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Call(c) = inst {
                    if in_lo && shared_lo.contains(c.func.as_str()) {
                        c.func = format!("{}_isr", c.func);
                    } else if in_hi && shared_hi.contains(c.func.as_str()) {
                        c.func = format!("{}_isr_high", c.func);
                    }
                }
            }
        }
    }
    let mut out: Vec<String> = used
        .iter()
        .filter(|u| shared_lo.contains(u.as_str()))
        .map(|u| format!("{u}_isr"))
        .collect();
    out.extend(
        used.iter()
            .filter(|u| shared_hi.contains(u.as_str()))
            .map(|u| format!("{u}_isr_high")),
    );
    out
}

/// Fresh SSA name supply for the fcmp materialization trees. Starts from
/// every name the module defines (params plus inst dsts, module-wide: names
/// need uniqueness inside a function only, so the conservative seed skips
/// a few candidates), then hands out `c0`, `c1`, … skipping taken names.
/// Hands out names in order, so the lowered text stays stable.
struct FreshNames {
    used: HashSet<String>,
    next: u64,
}

impl FreshNames {
    fn from_module(m: &Module) -> FreshNames {
        let mut used: HashSet<String> = HashSet::new();
        for f in &m.funcs {
            for p in &f.params {
                used.insert(p.name.clone());
            }
            for b in &f.blocks {
                for inst in &b.insts {
                    if let Some(dst) = inst_dst(inst) {
                        used.insert(dst.to_string());
                    }
                }
            }
        }
        FreshNames { used, next: 0 }
    }

    fn fresh(&mut self) -> String {
        loop {
            let n = format!("c{}", self.next);
            self.next += 1;
            if self.used.insert(n.clone()) {
                return n;
            }
        }
    }
}

/// The dst register defined by `inst`, if any.
fn inst_dst(inst: &Inst) -> Option<&str> {
    match inst {
        Inst::Load(l) => Some(&l.dst),
        Inst::Bin(b) => Some(&b.dst),
        Inst::Zext(z) => Some(&z.dst),
        Inst::Sext(s) => Some(&s.dst),
        Inst::Trunc(t) => Some(&t.dst),
        Inst::IntToPtr(p) => Some(&p.dst),
        Inst::Icmp(i) => Some(&i.dst),
        Inst::Select(s) => Some(&s.dst),
        Inst::Call(c) => c.dst.as_deref(),
        Inst::Phi(p) => Some(&p.dst),
        Inst::Gep(g) => Some(&g.dst),
        Inst::Alloca(a) => Some(&a.dst),
        Inst::Freeze(f) => Some(&f.dst),
        Inst::VaArg(v) => Some(&v.dst),
        Inst::VaStart(_) => None,
        Inst::FloatBin(b) => Some(&b.dst),
        Inst::Fcmp(c) => Some(&c.dst),
        Inst::FloatConv(c) => Some(&c.dst),
        Inst::Asm(_) => None,
        Inst::Ret(_, _)
        | Inst::Store(_)
        | Inst::Br(_)
        | Inst::BrCond(_)
        | Inst::Switch(_)
        | Inst::Memcpy(_) => None,
    }
}

/// The soft-float arithmetic routine for an f32 binop.
fn fbin_routine(op: FBinOp) -> &'static str {
    match op {
        FBinOp::FAdd => "__add_f32",
        FBinOp::FSub => "__sub_f32",
        FBinOp::FMul => "__mul_f32",
        FBinOp::FDiv => "__div_f32",
    }
}

/// Rewrite one `Inst::FloatBin` into the runtime call: dst/ty preserved,
/// both operands copied as f32 args.
fn lower_fbin(b: &ir::FloatBin, used: &mut Vec<String>) -> Inst {
    let func = fbin_routine(b.op);
    if !used.iter().any(|u| u == func) {
        used.push(func.to_string());
    }
    Inst::Call(Call {
        dst: Some(b.dst.clone()),
        ty: Some(Ty::F32),
        func: func.to_string(),
        args: vec![
            CallArg {
                ty: Some(Ty::F32),
                val: b.a.clone(),
                byval: None,
                sret: false,
            },
            CallArg {
                ty: Some(Ty::F32),
                val: b.b.clone(),
                byval: None,
                sret: false,
            },
        ],
        callees: Vec::new(),
        loc: b.loc.clone(),
    })
}

/// Rewrite one `Inst::Fcmp` into `%c = call i8 @__cmp_f32(a, b)` followed by
/// the per-predicate icmp/select tree that materializes the i1 result from
/// the tri-state byte. Returns the whole replacement sequence.
fn lower_fcmp(c: &ir::Fcmp, used: &mut Vec<String>, names: &mut FreshNames) -> Vec<Inst> {
    let func = "__cmp_f32";
    if !used.iter().any(|u| u == func) {
        used.push(func.to_string());
    }
    let call_dst = names.fresh();
    let mut insts = vec![Inst::Call(Call {
        dst: Some(call_dst.clone()),
        ty: Some(Ty::I8),
        func: func.to_string(),

        args: vec![
            CallArg {
                ty: Some(Ty::F32),
                val: c.a.clone(),
                byval: None,
                sret: false,
            },
            CallArg {
                ty: Some(Ty::F32),
                val: c.b.clone(),
                byval: None,
                sret: false,
            },
        ],
        callees: Vec::new(),
        loc: c.loc.clone(),
    })];
    insts.extend(fcmp_tree(&c.pred, &call_dst, &c.dst, names));
    insts
}

fn fcmp_icmp(pred: &str, c: &str, k: i64, dst: &str) -> Inst {
    Inst::Icmp(ir::Icmp {
        dst: dst.into(),
        pred: pred.into(),
        ty: Ty::I8,
        a: Val::Reg(c.into()),
        b: Val::Const(k),
        loc: None,
    })
}

/// The per-predicate materialization tree over the `__cmp_f32` tri-state
/// byte (0 = equal, 1 = a < b, 2 = a > b, 3 = unordered). Each tree is
/// either a single `icmp eq/ne i8 %c, <k>` or the OR of two equality
/// icmps materialized as `select i1 <c==k1>, i1 true, i1 <c==k2>`: no i1
/// binops (isel rejects them). The trees match the isel contract the
/// legalize tests document.
/// | predicate | tree |
/// |---|---|
/// | `oeq` | `(c==0)` |
/// | `ogt` | `(c==2)` |
/// | `oge` | `(c==2)\|\|(c==0)` |
/// | `olt` | `(c==1)` |
/// | `ole` | `(c==1)\|\|(c==0)` |
/// | `one` | `(c==1)\|\|(c==2)` |
/// | `ord` | `(c!=3)` |
/// | `ueq` | `(c==0)\|\|(c==3)` |
/// | `ugt` | `(c==2)\|\|(c==3)` |
/// | `uge` | `(c!=1)` |
/// | `ult` | `(c==1)\|\|(c==3)` |
/// | `ule` | `(c!=2)` |
/// | `une` | `(c!=0)` |
/// | `uno` | `(c==3)` |
///
/// `fcmp true`/`fcmp false` are compile-time constants (clang emits neither)
/// and panic instead of materializing a call the isel cannot remove: no
/// lowering path exists for them.
fn fcmp_tree(pred: &str, c: &str, dst: &str, names: &mut FreshNames) -> Vec<Inst> {
    fn or(c: &str, k1: i64, k2: i64, dst: &str, names: &mut FreshNames, out: &mut Vec<Inst>) {
        let t1 = names.fresh();
        let t2 = names.fresh();
        out.push(fcmp_icmp("eq", c, k1, &t1));
        out.push(fcmp_icmp("eq", c, k2, &t2));
        out.push(Inst::Select(ir::Select {
            dst: dst.into(),
            cond: Val::Reg(t1),
            ty: Ty::I1,
            a: Val::Const(1), // i1 true
            b: Val::Reg(t2),
            ptr: false,
            loc: None,
        }));
    }
    let mut out = Vec::new();
    match pred {
        "oeq" => out.push(fcmp_icmp("eq", c, 0, dst)),
        "ogt" => out.push(fcmp_icmp("eq", c, 2, dst)),
        "oge" => or(c, 2, 0, dst, names, &mut out),
        "olt" => out.push(fcmp_icmp("eq", c, 1, dst)),
        "ole" => or(c, 1, 0, dst, names, &mut out),
        "one" => or(c, 1, 2, dst, names, &mut out),
        "ord" => out.push(fcmp_icmp("ne", c, 3, dst)),
        "ueq" => or(c, 0, 3, dst, names, &mut out),
        "ugt" => or(c, 2, 3, dst, names, &mut out),
        "uge" => out.push(fcmp_icmp("ne", c, 1, dst)),
        "ult" => or(c, 1, 3, dst, names, &mut out),
        "ule" => out.push(fcmp_icmp("ne", c, 2, dst)),
        "une" => out.push(fcmp_icmp("ne", c, 0, dst)),
        "uno" => out.push(fcmp_icmp("eq", c, 3, dst)),
        "true" | "false" => {
            panic!("legalize: fcmp {pred} is a compile-time constant (clang never emits it)")
        }
        other => panic!("legalize: unknown fcmp predicate {other:?}"),
    }
    out
}

/// Rewrites one `Inst::FloatConv`. The int to float conversions become calls
/// to the four conversion routines: the source/target width rides on the
/// call's types (the routine's slot is always 4 bytes; an i8/i16 source or
/// result uses the low bytes). `fpext`/`fptrunc` are f32 to f32 (double equals
/// float on msp430) and become a plain `freeze` copy, no call. Anything
/// touching a non-f32 type is an f64 attempt and panics: no f64 path exists.
fn lower_fconv(c: &ir::FloatConv, used: &mut Vec<String>) -> Inst {
    fn mark(func: &'static str, used: &mut Vec<String>) -> String {
        if !used.iter().any(|u| u == func) {
            used.push(func.to_string());
        }
        func.to_string()
    }

    let dst = Some(c.dst.clone());
    match (c.op, c.from, c.to) {
        (FloatConvOp::FpToSi, Ty::F32, to @ (Ty::I8 | Ty::I16 | Ty::I32)) => Inst::Call(Call {
            dst,
            ty: Some(to),
            func: mark("__fptosi_f32", used),
            args: vec![CallArg { ty: Some(Ty::F32), val: c.val.clone(), byval: None, sret: false }],
            callees: Vec::new(),
            loc: None,
        }),
        (FloatConvOp::FpToUi, Ty::F32, to @ (Ty::I8 | Ty::I16 | Ty::I32)) => Inst::Call(Call {
            dst,
            ty: Some(to),
            func: mark("__fptoui_f32", used),
            args: vec![CallArg { ty: Some(Ty::F32), val: c.val.clone(), byval: None, sret: false }],
            callees: Vec::new(),
            loc: None,
        }),
        (FloatConvOp::SiToFp, from @ (Ty::I8 | Ty::I16 | Ty::I32), Ty::F32) => Inst::Call(Call {
            dst,
            ty: Some(Ty::F32),
            func: mark("__sitofp_f32", used),
            args: vec![CallArg { ty: Some(from), val: c.val.clone(), byval: None, sret: false }],
            callees: Vec::new(),
            loc: None,
        }),
        (FloatConvOp::UiToFp, from @ (Ty::I8 | Ty::I16 | Ty::I32), Ty::F32) => Inst::Call(Call {
            dst,
            ty: Some(Ty::F32),
            func: mark("__uitofp_f32", used),
            args: vec![CallArg { ty: Some(from), val: c.val.clone(), byval: None, sret: false }],
            callees: Vec::new(),
            loc: None,
        }),
        (FloatConvOp::Fpext | FloatConvOp::Fptrunc, Ty::F32, Ty::F32) => {
            Inst::Freeze(ir::Freeze { dst: c.dst.clone(), ty: Ty::F32, val: c.val.clone(), loc: None })
        }
        other => panic!(
            "legalize: unsupported float conversion {other:?} (msp430 has no f64; fpext/fptrunc are f32->f32 only)"
        ),
    }
}

/// Lowers `llvm.smax/smin/abs.W` to icmp/select trees.
/// smax: sgt+select, smin: slt+select, abs: slt0+sub+select.
/// Width-parametric (i8/i16/i32) via byte-generic isel. Abs flag
/// ignored (wraps INT_MIN for false, poison allows any value for true).
/// Unknown `llvm.*` panics: no lowering exists for it.
fn lower_intrinsic(c: &Call, names: &mut FreshNames, used_routines: &mut Vec<String>) -> Vec<Inst> {
    let dst = c
        .dst
        .clone()
        .unwrap_or_else(|| panic!("legalize: intrinsic {} must carry a dst", c.func));
    let ty =
        c.ty.unwrap_or_else(|| panic!("legalize: intrinsic {} must carry a result type", c.func));
    let cond = names.fresh();
    match c.func.as_str() {
        "llvm.smax.i8" | "llvm.smax.i16" | "llvm.smax.i32" => {
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "sgt".into(),
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a,
                    b,
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.smin.i8" | "llvm.smin.i16" | "llvm.smin.i32" => {
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "slt".into(),
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a,
                    b,
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.umin.i8" | "llvm.umin.i16" | "llvm.umin.i32" => {
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "ult".into(),
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a,
                    b,
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.umax.i8" | "llvm.umax.i16" | "llvm.umax.i32" => {
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "ugt".into(),
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a,
                    b,
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.usub.sat.i8" | "llvm.usub.sat.i16" | "llvm.usub.sat.i32" => {
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            let sub = names.fresh();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "uge".into(),
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Bin(ir::Bin {
                    dst: sub.clone(),
                    op: ir::BinOp::Sub,
                    ty,
                    a: a.clone(),
                    b: b.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a: Val::Reg(sub),
                    b: Val::Const(0),
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.abs.i8" | "llvm.abs.i16" | "llvm.abs.i32" => {
            let a = c.args[0].val.clone();
            // The second arg (is_int_min_poison) is read but ignored; see
            // the fn doc for why one lowering serves both flag values.
            let neg = names.fresh();
            vec![
                Inst::Icmp(ir::Icmp {
                    dst: cond.clone(),
                    pred: "slt".into(),
                    ty,
                    a: a.clone(),
                    b: Val::Const(0),
                    loc: None,
                }),
                Inst::Bin(ir::Bin {
                    dst: neg.clone(),
                    op: ir::BinOp::Sub,
                    ty,
                    a: Val::Const(0),
                    b: a.clone(),
                    loc: None,
                }),
                Inst::Select(ir::Select {
                    dst,
                    cond: Val::Reg(cond),
                    ty,
                    a: Val::Reg(neg),
                    b: a,
                    ptr: false,
                    loc: None,
                }),
            ]
        }
        "llvm.fshl.i8" | "llvm.fshl.i16" | "llvm.fshl.i32" | "llvm.fshr.i8" | "llvm.fshr.i16"
        | "llvm.fshr.i32" => {
            // Funnel-shift: fshl(a,b,c) = (a << (c mod N)) | (b >> ((N - (c mod N)) mod N))
            // with the 0-shift guard so the complementary shift is 0 not N.
            // For a const shift the masked count is const-folded so the two
            // funnel shifts stay as Bin with Const (isel inlines them). For a
            // variable shift the counts are Reg and must be Calls
            // (__shl/__lshr), as the main legalize loop does not re-run on the
            // Bins we create here, so we emit Calls directly.
            let a = c.args[0].val.clone();
            let b = c.args[1].val.clone();
            let sh = c.args[2].val.clone();
            let is_fshl = c.func.starts_with("llvm.fshl");
            let width: u8 = match ty {
                Ty::I8 => 8,
                Ty::I16 => 16,
                Ty::I32 => 32,
                _ => panic!("legalize: {} unsupported type {ty:?}", c.func),
            };
            let width_i64 = i64::from(width);
            let mask_i64 = i64::from(width - 1);
            let mut insts: Vec<Inst> = Vec::new();
            if let Val::Const(c) = sh.clone() {
                let sh_masked_c = c & mask_i64;
                let sub_c = (width_i64 - sh_masked_c) & mask_i64;
                let sh_val = Val::Const(sh_masked_c);
                let sub_val = Val::Const(sub_c);
                let shl_dst = names.fresh();
                let shr_dst = names.fresh();
                if is_fshl {
                    insts.push(Inst::Bin(ir::Bin {
                        dst: shl_dst.clone(),
                        op: BinOp::Shl,
                        ty,
                        a: a.clone(),
                        b: sh_val,
                        loc: None,
                    }));
                    if sh_masked_c == 0 {
                        insts.push(Inst::Freeze(ir::Freeze {
                            dst: shr_dst.clone(),
                            ty,
                            val: Val::Const(0),
                            loc: None,
                        }));
                    } else {
                        insts.push(Inst::Bin(ir::Bin {
                            dst: shr_dst.clone(),
                            op: BinOp::LShr,
                            ty,
                            a: b.clone(),
                            b: sub_val,
                            loc: None,
                        }));
                    }
                } else {
                    insts.push(Inst::Bin(ir::Bin {
                        dst: shr_dst.clone(),
                        op: BinOp::LShr,
                        ty,
                        a: a.clone(),
                        b: sh_val,
                        loc: None,
                    }));
                    if sh_masked_c == 0 {
                        insts.push(Inst::Freeze(ir::Freeze {
                            dst: shl_dst.clone(),
                            ty,
                            val: Val::Const(0),
                            loc: None,
                        }));
                    } else {
                        insts.push(Inst::Bin(ir::Bin {
                            dst: shl_dst.clone(),
                            op: BinOp::Shl,
                            ty,
                            a: b.clone(),
                            b: sub_val,
                            loc: None,
                        }));
                    }
                }
                insts.push(Inst::Bin(ir::Bin {
                    dst,
                    op: BinOp::Or,
                    ty,
                    a: Val::Reg(shl_dst),
                    b: Val::Reg(shr_dst),
                    loc: None,
                }));
                return insts;
            }
            let sh_masked = names.fresh();
            let sub_dst = names.fresh();
            let shl_dst = names.fresh();
            let shr_dst = names.fresh();
            let shr_sel = names.fresh();
            let is_zero = names.fresh();
            insts.push(Inst::Bin(ir::Bin {
                dst: sh_masked.clone(),
                op: BinOp::And,
                ty,
                a: sh.clone(),
                b: Val::Const(mask_i64),
                loc: None,
            }));
            insts.push(Inst::Bin(ir::Bin {
                dst: sub_dst.clone(),
                op: BinOp::Sub,
                ty,
                a: Val::Const(width_i64),
                b: Val::Reg(sh_masked.clone()),
                loc: None,
            }));
            if is_fshl {
                let func = match ty {
                    Ty::I8 => "__shl_u8",
                    Ty::I16 => "__shl_u16",
                    Ty::I32 => "__shl_u32",
                    _ => unreachable!(),
                };
                if !used_routines.iter().any(|u| u == func) {
                    used_routines.push(func.to_string());
                }
                insts.push(Inst::Call(Call {
                    dst: Some(shl_dst.clone()),
                    ty: Some(ty),
                    func: func.to_string(),
                    args: vec![
                        CallArg {
                            ty: Some(ty),
                            val: a.clone(),
                            byval: None,
                            sret: false,
                        },
                        CallArg {
                            ty: Some(ty),
                            val: Val::Reg(sh_masked.clone()),
                            byval: None,
                            sret: false,
                        },
                    ],
                    callees: Vec::new(),
                    loc: None,
                }));
                let func = match ty {
                    Ty::I8 => "__lshr_u8",
                    Ty::I16 => "__lshr_u16",
                    Ty::I32 => "__lshr_u32",
                    _ => unreachable!(),
                };
                if !used_routines.iter().any(|u| u == func) {
                    used_routines.push(func.to_string());
                }
                insts.push(Inst::Call(Call {
                    dst: Some(shr_dst.clone()),
                    ty: Some(ty),
                    func: func.to_string(),
                    args: vec![
                        CallArg {
                            ty: Some(ty),
                            val: b.clone(),
                            byval: None,
                            sret: false,
                        },
                        CallArg {
                            ty: Some(ty),
                            val: Val::Reg(sub_dst.clone()),
                            byval: None,
                            sret: false,
                        },
                    ],
                    callees: Vec::new(),
                    loc: None,
                }));
            } else {
                let func = match ty {
                    Ty::I8 => "__lshr_u8",
                    Ty::I16 => "__lshr_u16",
                    Ty::I32 => "__lshr_u32",
                    _ => unreachable!(),
                };
                if !used_routines.iter().any(|u| u == func) {
                    used_routines.push(func.to_string());
                }
                insts.push(Inst::Call(Call {
                    dst: Some(shr_dst.clone()),
                    ty: Some(ty),
                    func: func.to_string(),
                    args: vec![
                        CallArg {
                            ty: Some(ty),
                            val: a.clone(),
                            byval: None,
                            sret: false,
                        },
                        CallArg {
                            ty: Some(ty),
                            val: Val::Reg(sh_masked.clone()),
                            byval: None,
                            sret: false,
                        },
                    ],
                    callees: Vec::new(),
                    loc: None,
                }));
                let func = match ty {
                    Ty::I8 => "__shl_u8",
                    Ty::I16 => "__shl_u16",
                    Ty::I32 => "__shl_u32",
                    _ => unreachable!(),
                };
                if !used_routines.iter().any(|u| u == func) {
                    used_routines.push(func.to_string());
                }
                insts.push(Inst::Call(Call {
                    dst: Some(shl_dst.clone()),
                    ty: Some(ty),
                    func: func.to_string(),
                    args: vec![
                        CallArg {
                            ty: Some(ty),
                            val: b.clone(),
                            byval: None,
                            sret: false,
                        },
                        CallArg {
                            ty: Some(ty),
                            val: Val::Reg(sub_dst.clone()),
                            byval: None,
                            sret: false,
                        },
                    ],
                    callees: Vec::new(),
                    loc: None,
                }));
            }
            insts.push(Inst::Icmp(ir::Icmp {
                dst: is_zero.clone(),
                pred: "eq".into(),
                ty,
                a: Val::Reg(sh_masked.clone()),
                b: Val::Const(0),
                loc: None,
            }));
            insts.push(Inst::Select(ir::Select {
                dst: shr_sel.clone(),
                cond: Val::Reg(is_zero),
                ty,
                a: Val::Const(0),
                b: Val::Reg(shr_dst),
                ptr: false,
                loc: None,
            }));
            insts.push(Inst::Bin(ir::Bin {
                dst,
                op: BinOp::Or,
                ty,
                a: Val::Reg(shl_dst),
                b: Val::Reg(shr_sel),
                loc: None,
            }));
            insts
        }
        other => panic!("legalize: unknown intrinsic {other:?}"),
    }
}

/// Every function transitively reachable from `roots` over the caller to
/// callee map `adj` (the roots included). A visited set keeps a call cycle
/// (rejected later by callgraph/alloc) from looping forever.
fn reachable(roots: &[&str], adj: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<&str> = roots.to_vec();
    while let Some(f) = stack.pop() {
        if !seen.insert(f.to_string()) {
            continue;
        }
        if let Some(next) = adj.get(f) {
            stack.extend(next.iter().map(String::as_str));
        }
    }
    seen
}
/// The field offset sentinel for a read that touches every field of a
/// global (a whole-object access: memcpy source, or a base-pointer read the
/// GEP walk cannot narrow). A store edge only feeds an ISR read when the
/// stored field overlaps the read field; the sentinel overlaps every field.
const ALL_FIELDS: u16 = 0xFFFF;

/// Resolves a pointer operand to `(global, field_byte_offset)`, one hop
/// through GEPs. The field is the accumulated constant byte offset modulo
/// the innermost runtime scale: `gep @g +0 +10*%i +9` resolves to field 9,
/// `gep @g +0 +10*%i +0` to field 0, and a bare `@g`/`gep @g +0` to field 0.
/// A whole-object read at an unresolvable offset falls back to `ALL_FIELDS`
/// (a store into any field then feeds it); a runtime register base (a param,
/// an alloca) resolves to nothing and the store-edge scan treats it as
/// opaque. A register loaded from a global POINTER variable resolves
/// through `aliases` (built by `global_ptr_aliases`) when that variable's
/// only assignment is another global's address (the HAL `g_handle =
/// &g_storage;` idiom, epic-cc#463): the walk continues from the aliased
/// global exactly as if the load's pointer operand had been a GEP of it.
fn global_field(ptr: &str, f: &Func, aliases: &HashMap<String, String>) -> Option<(String, u16)> {
    if let Some(g) = ptr.strip_prefix('@') {
        return Some((g.to_string(), 0));
    }
    let mut cur = ptr.strip_prefix('%')?.to_string();
    let mut k: u16 = 0;
    let mut min_scale: Option<u16> = None;
    for _ in 0..8 {
        let mut found = false;
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Gep(g) = inst {
                    if g.dst == cur {
                        match &g.base {
                            GepBase::Global(n) => {
                                k = k.wrapping_add(u16::from(g.k));
                                if min_scale.is_none() {
                                    min_scale = g.terms.iter().map(|(s, _)| *s).min();
                                }
                                return Some((n.clone(), field_of(k, min_scale)));
                            }
                            GepBase::Reg(r) => {
                                k = k.wrapping_add(u16::from(g.k));
                                if min_scale.is_none() {
                                    min_scale = g.terms.iter().map(|(s, _)| *s).min();
                                }
                                cur = r.clone();
                                found = true;
                                break;
                            }
                        }
                    }
                }
                if let Inst::Load(l) = inst {
                    if l.dst == cur {
                        if let Some(g_ptr) = l.ptr.strip_prefix('@') {
                            if let Some(target) = aliases.get(g_ptr) {
                                return Some((target.clone(), field_of(k, min_scale)));
                            }
                        }
                    }
                }
            }
            if found {
                break;
            }
        }
        if !found {
            return None;
        }
    }
    None
}

/// Global pointer variables whose only role is holding another global's
/// address (`g_handle = &g_storage;`, IR `store i16 @g_storage @g_handle`):
/// `g_handle -> g_storage`. A pointer variable assigned two different
/// globals anywhere is dropped (ambiguous, opaque like any other unresolved
/// case) rather than picking one arbitrarily.
fn global_ptr_aliases(m: &Module) -> HashMap<String, String> {
    let globals: HashSet<&str> = m.globals.iter().map(|g| g.name.as_str()).collect();
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Store(s) = inst {
                    let Some(g_ptr) = s.ptr.strip_prefix('@') else {
                        continue;
                    };
                    let Val::Global(target) = &s.val else {
                        continue;
                    };
                    if !globals.contains(target.as_str()) {
                        continue;
                    }
                    match aliases.get(g_ptr) {
                        Some(prev) if prev != target => {
                            ambiguous.insert(g_ptr.to_string());
                        }
                        _ => {
                            aliases.insert(g_ptr.to_string(), target.clone());
                        }
                    }
                }
            }
        }
    }
    for g in ambiguous {
        aliases.remove(&g);
    }
    aliases
}

/// The field a constant byte offset selects, given the innermost runtime
/// scale: `offset % scale` when a scale is known (an array-of-struct
/// element stride), the bare offset otherwise (a direct struct or global).
fn field_of(offset: u16, min_scale: Option<u16>) -> u16 {
    match min_scale {
        Some(s) if s > 0 => offset % u16::from(s),
        _ => offset,
    }
}

/// `global_field` over a precomputed per-function GEP base map (the
/// store-rewrite loop mutates the function, so resolution must not borrow
/// it). The GEP's constant offset `k` and term scales ride in the map
/// entries.
fn global_field_map(
    ptr: &str,
    bases: &HashMap<String, (GepBase, u16, Vec<(u16, String)>)>,
) -> Option<(String, u16)> {
    if let Some(g) = ptr.strip_prefix('@') {
        return Some((g.to_string(), 0));
    }
    let mut cur = ptr.strip_prefix('%')?.to_string();
    let mut k: u16 = 0;
    let mut min_scale: Option<u16> = None;
    for _ in 0..8 {
        match bases.get(&cur) {
            Some((GepBase::Global(n), gk, terms)) => {
                k = k.wrapping_add(u16::from(*gk));
                if min_scale.is_none() {
                    min_scale = terms.iter().map(|(s, _)| *s).min();
                }
                return Some((n.clone(), field_of(k, min_scale)));
            }
            Some((GepBase::Reg(r), gk, terms)) => {
                k = k.wrapping_add(u16::from(*gk));
                if min_scale.is_none() {
                    min_scale = terms.iter().map(|(s, _)| *s).min();
                }
                cur = r.clone();
            }
            None => return None,
        }
    }
    None
}

/// Like `global_field_map` but for a LOCAL base: walks `ptr`'s GEP chain
/// through `bases` and returns the ultimate register it is rooted at (its
/// own name if `ptr` is already an unresolved register), instead of giving
/// up when the chain never reaches a global. The caller checks the
/// returned register against a known set of alloca registers (e.g. a
/// memcpy source, epic-cc#463); an opaque non-alloca root simply never
/// matches that set, so this stays as safe as `global_field_map`'s "opaque"
/// fallback elsewhere in this pass.
fn alloca_root_map(
    ptr: &str,
    bases: &HashMap<String, (GepBase, u16, Vec<(u16, String)>)>,
) -> Option<String> {
    let mut cur = ptr.strip_prefix('%')?.to_string();
    for _ in 0..8 {
        match bases.get(&cur) {
            Some((GepBase::Reg(r), _, _)) => cur = r.clone(),
            Some((GepBase::Global(_), _, _)) => return None,
            None => return Some(cur),
        }
    }
    None
}

/// GEP bases plus Load-from-aliased-global entries: a register loaded
/// from a global pointer variable holding another global's address
/// (`g_handle = &g_storage`) resolves as that global with offset 0, so
/// pointer-carried chains (`%o = load @g_handle; %p = gep %o +k`)
/// settle in the map-based arms exactly as in `global_field`
/// (epic-cc#642). The four `bases` construction sites share this.
fn bases_with_alias_loads(
    f: &Func,
    aliases: &HashMap<String, String>,
) -> HashMap<String, (GepBase, u16, Vec<(u16, String)>)> {
    let mut bases: HashMap<String, (GepBase, u16, Vec<(u16, String)>)> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                Inst::Gep(g) => {
                    bases.insert(g.dst.clone(), (g.base.clone(), g.k, g.terms.clone()));
                }
                Inst::Load(l) => {
                    if let Some(g_ptr) = l.ptr.strip_prefix('@') {
                        if let Some(target) = aliases.get(g_ptr) {
                            bases.insert(
                                l.dst.clone(),
                                (GepBase::Global(target.clone()), 0, Vec::new()),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    bases
}

/// The `@g`/`%r` pointer text of a `Val` (memcpy operands are pointer
/// values). A non-pointer val has no pointer text.
fn ptr_of_val(v: &Val) -> String {
    match v {
        Val::Global(g) => format!("@{g}"),
        Val::Reg(r) => format!("%{r}"),
        _ => String::new(),
    }
}

/// Whether `ptr` (a GEP chain, one hop like `global_field`) resolves to a
/// field of `alloca_reg`'s own object, or to `alloca_reg` itself. Mirrors
/// `global_field`'s walk but the root is a local alloca register instead of
/// a global, so a memcpy's source struct can be matched against the field
/// stores that built it (epic-cc#463).
fn alloca_field(ptr: &str, f: &Func, alloca_reg: &str) -> Option<u16> {
    if ptr.strip_prefix('%') == Some(alloca_reg) {
        return Some(0);
    }
    let mut cur = ptr.strip_prefix('%')?.to_string();
    let mut k: u16 = 0;
    for _ in 0..8 {
        let mut found = false;
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Gep(g) = inst {
                    if g.dst == cur {
                        let GepBase::Reg(r) = &g.base else {
                            return None;
                        };
                        k = k.wrapping_add(u16::from(g.k));
                        if r == alloca_reg {
                            return Some(k);
                        }
                        cur = r.clone();
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
        if !found {
            return None;
        }
    }
    None
}

/// The `(global, field)` pairs the ISR context reads (loads, memcpy
/// sources): a store into one of these feeds an ISR indirect call, so the
/// stored value rewrites to the `_isr` copy when duplicated. A global the
/// ISR only writes feeds no ISR call, so stores into it stay on the original.
/// Field-sensitive: the scheduler task table stores fn/arg fields from main
/// while the ISR tick reads other fields, so those stored functions stay out
/// of the ISR context and main-context dispatch keeps its candidates.
/// A memcpy source (whole object) reads `ALL_FIELDS` (epic-cc#73) (epic-hal#86).
fn isr_read_globals(
    m: &Module,
    isr_ctx: &HashSet<String>,
    aliases: &HashMap<String, String>,
) -> HashSet<(String, u16)> {
    let mut out: HashSet<(String, u16)> = HashSet::new();
    for f in &m.funcs {
        if !isr_ctx.contains(&f.name) {
            continue;
        }
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Load(l) => {
                        if let Some((g, k)) = global_field(&l.ptr, f, aliases) {
                            out.insert((g, k));
                        }
                    }
                    Inst::Memcpy(mc) => {
                        if let Some((g, _)) = global_field(&ptr_of_val(&mc.src), f, aliases) {
                            out.insert((g, ALL_FIELDS));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// Caller -> callee adjacency over direct calls and address-taken
/// globals, plus the defined-function set. Priority-independent: every
/// context (main, low-ISR, high-ISR) derives from this one graph.
fn isr_adjacency(m: &Module) -> (HashMap<String, Vec<String>>, HashSet<String>) {
    let defined: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for f in &m.funcs {
        adj.entry(f.name.clone()).or_default();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    if c.callees.is_empty() {
                        adj.entry(f.name.clone()).or_default().push(c.func.clone());
                    }
                }
                let mut taken = HashSet::new();
                collect_global_vals(inst, &mut taken);
                for g in taken {
                    adj.entry(f.name.clone()).or_default().push(g);
                }
            }
        }
    }
    (adj, defined)
}

/// Function-valued stores into `handle` (any field, any function) join the
/// ISR context: the handle is memcpy'd whole-object into an ISR-read
/// storage global, so field sensitivity would be false precision (the same
/// stance as the alloca path). Returns whether anything new joined.
fn join_stores_into_global(
    m: &Module,
    handle: &str,
    aliases: &HashMap<String, String>,
    defined: &HashSet<String>,
    isr_ctx: &mut HashSet<String>,
) -> bool {
    let mut grew = false;
    for sf in &m.funcs {
        for sb in &sf.blocks {
            for si in &sb.insts {
                let Inst::Store(s) = si else { continue };
                let Some((g, _)) = global_field(&s.ptr, sf, aliases) else {
                    continue;
                };
                if g != handle {
                    continue;
                }
                if let Val::Global(fn_name) = &s.val {
                    if defined.contains(fn_name.as_str()) && isr_ctx.insert(fn_name.clone()) {
                        grew = true;
                    }
                }
            }
        }
    }
    grew
}

/// The globals passed as argument `pi` to `f` across every call site: the
/// callers' handle globals behind a memcpy whose source is `f`'s param
/// (the `Init(&h)` idiom before clang promotes the param).
fn globals_passed_as_param(funcs: &[Func], f_name: &str, pi: usize) -> Vec<String> {
    let mut out = Vec::new();
    for cf in funcs {
        for cb in &cf.blocks {
            for ci in &cb.insts {
                if let Inst::Call(c) = ci {
                    if c.func == f_name {
                        if let Some(Val::Global(g)) = c.args.get(pi).map(|a| &a.val) {
                            out.push(g.clone());
                        }
                    }
                }
            }
        }
    }
    out
}

/// Extended context for ONE ISR priority root set: reachability over
/// direct calls and address-value edges, plus the store-edge fixpoint
/// (a defined function stored into a context-read global joins, with
/// its callees transitively). Returns `(ctx, read, param_stores)`.
/// Callers pass the roots of one priority; the union over priorities
/// is NOT the same (a store feeding only one priority's read set joins
/// only that priority), so each priority computes its own.
fn isr_context_for(
    m: &Module,
    roots: &HashSet<&str>,
    adj: &HashMap<String, Vec<String>>,
    defined: &HashSet<String>,
) -> (
    HashSet<String>,
    HashSet<(String, u16)>,
    HashSet<(String, usize, String)>,
) {
    let mut isr_ctx: HashSet<String> = roots.iter().flat_map(|r| reachable(&[*r], adj)).collect();
    let mut param_stores: HashSet<(String, usize, String)> = HashSet::new();
    let aliases = global_ptr_aliases(m);
    // Store edges, iterated to a fixpoint: a defined function (or a param
    // resolved through call sites) stored into a global the ISR context
    // READS joins the ISR context, and its own callees join transitively
    // (a cross-context callback that calls a shared helper must have the
    // helper duplicated too, or `cb_isr` would call the main-context
    // original and clobber a live main frame, ADR-013). The read set is
    // recomputed over the grown context each round, so a global read only
    // by a store-edge-added function is still seen.
    loop {
        let read = isr_read_globals(m, &isr_ctx, &aliases);
        let mut grew = false;
        for f in &m.funcs {
            for b in &f.blocks {
                for inst in &b.insts {
                    if let Inst::Store(s) = inst {
                        let Some((g, sk)) = global_field(&s.ptr, f, &aliases) else {
                            continue;
                        };
                        let feeds = read
                            .iter()
                            .any(|(rg, rk)| *rg == g && (*rk == ALL_FIELDS || *rk == sk));
                        if !feeds {
                            continue;
                        }
                        match &s.val {
                            Val::Global(fn_name) if defined.contains(fn_name.as_str()) => {
                                if isr_ctx.insert(fn_name.clone()) {
                                    grew = true;
                                }
                            }
                            Val::Reg(reg_name) => {
                                // Param hop: any call of this function
                                // passing a named function as this param
                                // feeds the global.
                                let param_idx =
                                    f.params.iter().position(|param| param.name == *reg_name);
                                if let Some(pi) = param_idx {
                                    param_stores.insert((f.name.clone(), pi, g.clone()));
                                    for cf in &m.funcs {
                                        for cb in &cf.blocks {
                                            for ci in &cb.insts {
                                                if let Inst::Call(c) = ci {
                                                    if c.func == f.name {
                                                        if let Some(a) = c.args.get(pi) {
                                                            if let Val::Global(fn_name) = &a.val {
                                                                if defined
                                                                    .contains(fn_name.as_str())
                                                                    && isr_ctx
                                                                        .insert(fn_name.clone())
                                                                {
                                                                    grew = true;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    // A memcpy of a local struct into an ISR-read global: the
                    // HAL "Init(&h)" idiom builds a local handle via field
                    // stores then memcpy's the whole struct into a global the
                    // ISR reads (e.g. `g_t0_storage = *h;`). The direct-store
                    // case above misses this, since the store that actually
                    // writes the function targets the LOCAL alloca, not the
                    // global; any function-valued field of that alloca joins
                    // the ISR context here instead (epic-cc#463).
                    if let Inst::Memcpy(mc) = inst {
                        let Some((g, _)) = global_field(&ptr_of_val(&mc.dst), f, &aliases) else {
                            continue;
                        };
                        // The destination is written whole-object, not just
                        // the one field `global_field`'s 0-offset default
                        // reports for a bare `@g`, so a read of ANY field of
                        // `g` means this write may feed it: unlike the
                        // field-sensitive direct-store case above, which
                        // knows exactly which field it touches.
                        let feeds = read.iter().any(|(rg, _)| *rg == g);
                        if !feeds {
                            continue;
                        }
                        // The source struct carries the callback into the
                        // ISR-read storage; resolve it by shape: an alloca
                        // the caller field-stored (epic-cc#463), the handle
                        // as a file-scope static (`memcpy @g_storage, @h`,
                        // clang promoting Init's param), or that same
                        // param un-promoted. All three were a silent hole
                        // before epic-cc#484.
                        let src_ptr = ptr_of_val(&mc.src);
                        if let Some(src_reg) = src_ptr.strip_prefix('%').map(str::to_string) {
                            let is_alloca = f
                                .blocks
                                .iter()
                                .flat_map(|b| &b.insts)
                                .any(|i| matches!(i, Inst::Alloca(a) if a.dst == src_reg));
                            if is_alloca {
                                for b2 in &f.blocks {
                                    for inst2 in &b2.insts {
                                        let Inst::Store(s2) = inst2 else { continue };
                                        if alloca_field(&s2.ptr, f, &src_reg).is_none() {
                                            continue;
                                        }
                                        if let Val::Global(fn_name) = &s2.val {
                                            if defined.contains(fn_name.as_str())
                                                && isr_ctx.insert(fn_name.clone())
                                            {
                                                grew = true;
                                            }
                                        }
                                    }
                                }
                                continue;
                            }
                            if let Some(pi) = f.params.iter().position(|p| p.name == src_reg) {
                                for handle in globals_passed_as_param(&m.funcs, &f.name.clone(), pi)
                                {
                                    if join_stores_into_global(
                                        m,
                                        &handle,
                                        &aliases,
                                        defined,
                                        &mut isr_ctx,
                                    ) {
                                        grew = true;
                                    }
                                }
                            }
                            continue;
                        }
                        if let Some((src_g, _)) = global_field(&src_ptr, f, &aliases) {
                            if join_stores_into_global(m, &src_g, &aliases, defined, &mut isr_ctx) {
                                grew = true;
                            }
                        }
                    }
                }
            }
        }
        if !grew {
            break;
        }
        // The context grew: close it over callees so a store-edge-added
        // function's own callees join too. The closure runs from every
        // current member (not just the ISR roots), so the store-edge
        // additions are preserved and their callees join.
        let members: Vec<&str> = isr_ctx.iter().map(String::as_str).collect();
        let grown = reachable(&members, &adj);
        if grown.len() == isr_ctx.len() && grown.is_subset(&isr_ctx) {
            break;
        }
        isr_ctx = grown;
    }
    let read = isr_read_globals(m, &isr_ctx, &aliases);

    (isr_ctx, read, param_stores)
}

/// The interrupt shared-function duplication (the interrupt duplication):
/// every function reachable from both the ISR context (the ISR plus its
/// transitive callees) and the main context (main plus its transitive callees)
/// gets an `_isr` copy: a deep clone of the Func, renamed `{name}_isr`, with
/// the `isr` flag cleared (the copy is an ordinary function, not a second
/// vector entry). Every call inside the ISR context whose target is a
/// duplicated function rewrites to the `_isr` name, so the whole ISR context
/// runs against the copies. The originals stay main-context-only.
///
/// The call graph re-derives locally from the module's CALL insts rather
/// than depending on the callgraph crate: the scan is tiny and stable, and
/// legalize already owns the module (no new dependency).
/// Copy suffix for a priority's duplicated shared callees: the low
/// context (and the compatibility single-vector mode) keeps the
/// historical `_isr`; the high context, which can preempt the low one
/// mid-call, gets `_isr_high` so its frames never overlap the low's.
const LO_SUFFIX: &str = "_isr";
const HI_SUFFIX: &str = "_isr_high";
/// The spelling a storage global holds after the cross-context rewrite:
/// storage -> [(dispatched name, owning priority)]. Dispatch sites scope
/// their candidate lists through this map, so a site never lists a frame
/// from the other priority's region (ADR-038).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpellPrio {
    Lo,
    Hi,
    Main,
}

fn duplicate_isr_shared(
    m: Module,
) -> (
    Module,
    HashSet<String>,
    HashSet<String>,
    HashMap<String, Vec<(String, SpellPrio, u16)>>,
) {
    // Originals whose stored address was rewritten to this priority's
    // copy: the storage they were stored into is read by that priority,
    // so every OTHER context reading the same storage now holds the
    // copy's address and its dispatch sites must list it (epic-cc#568).
    let mut stored_lo: HashSet<String> = HashSet::new();
    let mut stored_hi: HashSet<String> = HashSet::new();
    let mut spellings: HashMap<String, Vec<(String, SpellPrio, u16)>> = HashMap::new();
    // Storage keys are canonicalized through the pointer aliases so a
    // store into `g_handle = &g_storage` records under the same key the
    // dispatch site's load resolves to.
    let aliases = global_ptr_aliases(&m);
    // Partition ISR roots by priority: 1 = high, anything else ISR =
    // low (priority 0 is the compatibility single-vector mode).
    let hi_roots: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority == 1)
        .map(|f| f.name.as_str())
        .collect();
    let lo_roots: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority != 1)
        .map(|f| f.name.as_str())
        .collect();
    if hi_roots.is_empty() && lo_roots.is_empty() {
        return (m, stored_lo, stored_hi, spellings);
    }
    // Mode validation: each vector owns at most one handler, and the
    // compatibility single-vector mode (priority 0) never mixes with
    // explicit priorities. Anything else has no sound wiring (two bodies
    // cannot share one vector entry), so panic here rather than emit a
    // silently broken image downstream.
    let compat_roots: Vec<&&str> = lo_roots
        .iter()
        .filter(|r| {
            m.funcs
                .iter()
                .find(|f| f.name.as_str() == **r)
                .is_some_and(|f| f.irq_priority == 0)
        })
        .collect();
    let explicit_lo = lo_roots.len() - compat_roots.len();
    assert!(
        hi_roots.len() <= 1,
        "legalize: two high-priority interrupt handlers (only one high vector exists)"
    );
    assert!(
        explicit_lo <= 1,
        "legalize: two low-priority interrupt handlers (only one low vector exists)"
    );
    assert!(
        compat_roots.len() <= 1,
        "legalize: two compatibility-mode interrupt handlers (only one vector exists)"
    );
    assert!(
        compat_roots.is_empty() || (hi_roots.is_empty() && explicit_lo == 0),
        "legalize: a compatibility-mode ISR cannot mix with explicit-priority ISRs (use __interrupt(1)/__interrupt(2) for every handler, or a single plain __interrupt handler)"
    );

    // The extended contexts (epic-cc#137), computed per priority over
    // one shared adjacency: a helper reachable from a priority's roots
    // (directly, address-taken, or via the store-edge fixpoint) belongs
    // to that priority's context. The main context derives from the
    // same adjacency.
    let (adj, defined) = isr_adjacency(&m);
    let main_ctx = reachable(&["main"], &adj);
    let (lo_ctx, lo_read, lo_params) = isr_context_for(&m, &lo_roots, &adj, &defined);
    let (hi_ctx, hi_read, hi_params) = isr_context_for(&m, &hi_roots, &adj, &defined);
    // main stays out of the duplication above, so an ISR that (transitively)
    // calls main would leave the ISR's call on the original `main`,
    // re-entering the main context and collapsing the disjoint-region
    // guarantee. Panics rather than miscompiling: re-entrant main has no lowering.
    assert!(
        !lo_ctx.contains("main"),
        "isel/legalize: the low-ISR context must not reach main; re-entrant main is unsupported"
    );
    assert!(
        !hi_ctx.contains("main"),
        "isel/legalize: the high-ISR context must not reach main; re-entrant main is unsupported"
    );
    // A helper shared with (reachable from) another live context needs
    // that context's own copy: the high ISR can preempt the low one
    // (and main) mid-call, so frames must be disjoint along every
    // preemption edge. In compatibility mode the high sets are empty
    // and this is exactly the old single-context rule. ISR roots and
    // `main` itself are never copied, only shared callees.
    let is_root_or_main = |f: &Func| f.isr || f.name == "main";
    let shared_lo: Vec<String> = m
        .funcs
        .iter()
        .filter(|f| !is_root_or_main(f))
        .filter(|f| {
            lo_ctx.contains(&f.name) && (main_ctx.contains(&f.name) || hi_ctx.contains(&f.name))
        })
        .map(|f| f.name.clone())
        .collect();
    let shared_hi: Vec<String> = m
        .funcs
        .iter()
        .filter(|f| !is_root_or_main(f))
        .filter(|f| {
            hi_ctx.contains(&f.name) && (main_ctx.contains(&f.name) || lo_ctx.contains(&f.name))
        })
        .map(|f| f.name.clone())
        .collect();
    if shared_lo.is_empty() && shared_hi.is_empty() {
        return (m, stored_lo, stored_hi, spellings);
    }

    // Deep-clone each shared func with its priority's suffix (renamed,
    // `isr` flag cleared, priority reset: a copy is an ordinary
    // function, not a second vector entry). A name collision with an
    // existing function panics loudly.
    fn make_copies(funcs: &[Func], shared: &[String], suffix: &str) -> Vec<Func> {
        let mut copies: Vec<Func> = Vec::with_capacity(shared.len());
        for name in shared {
            let copy_name = format!("{name}{suffix}");
            assert!(
                !funcs.iter().any(|f| f.name == copy_name),
                "legalize: duplicate-interrupt name collision: {copy_name} already exists"
            );
            let f = funcs
                .iter()
                .find(|f| &f.name == name)
                .expect("legalize: shared function vanished");
            let mut c = f.clone();
            c.name = copy_name;
            c.isr = false;
            c.irq_priority = 0;
            copies.push(c);
        }
        copies
    }
    /// Rewrite direct call targets and function-pointer values inside
    /// `rewrite_set` from duplicated originals to their `suffix` copies.
    /// Every rewritten address VALUE (a store into a global, a forwarded
    /// call argument) is recorded in `stored`: the storage it lands in is
    /// read by this priority, but nothing keeps OTHER contexts that
    /// dispatch through the same storage from holding the copy's address,
    /// so the candidate filler must know the rewrite happened (epic-cc#568).
    fn rewrite_calls_to_copies(
        funcs: &mut [Func],
        rewrite_set: &HashSet<String>,
        shared: &HashSet<&str>,
        suffix: &str,
        stored: &mut HashSet<String>,
    ) {
        for f in funcs.iter_mut() {
            if !rewrite_set.contains(&f.name) {
                continue;
            }
            for b in &mut f.blocks {
                for inst in &mut b.insts {
                    if let Inst::Call(c) = inst {
                        let target = c.func.clone();
                        if shared.contains(target.as_str()) {
                            c.func = format!("{target}{suffix}");
                        }
                    }
                    rewrite_inst_vals(inst, shared, suffix, stored);
                }
            }
        }
    }
    let mut funcs = m.funcs;
    let lo_copies = make_copies(&funcs, &shared_lo, LO_SUFFIX);
    let hi_copies = make_copies(&funcs, &shared_hi, HI_SUFFIX);
    let shared_lo_set: HashSet<&str> = shared_lo.iter().map(String::as_str).collect();
    let shared_hi_set: HashSet<&str> = shared_hi.iter().map(String::as_str).collect();
    let mut rewrite_lo: HashSet<String> = lo_ctx
        .iter()
        .filter(|n| n.as_str() != "main" && !shared_lo_set.contains(n.as_str()))
        .cloned()
        .collect();
    for c in &lo_copies {
        rewrite_lo.insert(c.name.clone());
    }
    let mut rewrite_hi: HashSet<String> = hi_ctx
        .iter()
        .filter(|n| n.as_str() != "main" && !shared_hi_set.contains(n.as_str()))
        .cloned()
        .collect();
    for c in &hi_copies {
        rewrite_hi.insert(c.name.clone());
    }
    // The copies go into the module before the rewrite so their internal
    // calls are rewritten too (a copy's call to another shared function ->
    // its suffixed copy, transitively).
    let mut copies = lo_copies;
    copies.extend(hi_copies);
    funcs.extend(copies);
    rewrite_calls_to_copies(
        &mut funcs,
        &rewrite_lo,
        &shared_lo_set,
        LO_SUFFIX,
        &mut stored_lo,
    );
    rewrite_calls_to_copies(
        &mut funcs,
        &rewrite_hi,
        &shared_hi_set,
        HI_SUFFIX,
        &mut stored_hi,
    );
    // Cross-context store rewrite (epic-cc#137), per priority: a store
    // of a duplicated function targets the copy of the priority that
    // READS the global (otherwise the ISR dispatches the main-context
    // address into main frames). Write-only globals stay on the
    // original (epic-cc#73). A store feeding BOTH priorities has no
    // single spelling: panic, don't miscompile.
    // A handle global memcpy'd whole-object into a priority-read storage
    // global (the `Init(&h)` idiom with a file-scope static handle,
    // epic-cc#484): a function-valued store into any field of the handle
    // feeds the storage global like a direct store would, so it needs the
    // same cross-context rewrite. The memcpy source resolves by shape: a
    // global (the promoted idiom) or a param pointing at the caller's
    // global; the alloca form stays per-function below (epic-cc#463).
    // Module-wide: the memcpy lives in Init, the stores in main.
    let mut handle_feeds_lo: HashSet<String> = HashSet::new();
    let mut handle_feeds_hi: HashSet<String> = HashSet::new();
    for sf in funcs.iter() {
        let bases = bases_with_alias_loads(sf, &aliases);
        for b in &sf.blocks {
            for inst in &b.insts {
                let Inst::Memcpy(mc) = inst else { continue };
                let Some((g, _)) = global_field_map(&ptr_of_val(&mc.dst), &bases) else {
                    continue;
                };
                let src_ptr = ptr_of_val(&mc.src);
                let mut handles: Vec<String> = Vec::new();
                if let Some((sg, _)) = global_field_map(&src_ptr, &bases) {
                    handles.push(sg);
                } else if let Some(src_reg) = src_ptr.strip_prefix('%') {
                    if let Some(pi) = sf.params.iter().position(|p| &p.name == src_reg) {
                        for cf in funcs.iter() {
                            for cb in &cf.blocks {
                                for ci in &cb.insts {
                                    if let Inst::Call(c) = ci {
                                        if c.func == sf.name {
                                            if let Some(Val::Global(h)) =
                                                c.args.get(pi).map(|a| &a.val)
                                            {
                                                handles.push(h.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                for h in handles {
                    if lo_read.iter().any(|(rg, _)| *rg == g) {
                        handle_feeds_lo.insert(h.clone());
                    }
                    if hi_read.iter().any(|(rg, _)| *rg == g) {
                        handle_feeds_hi.insert(h.clone());
                    }
                }
            }
        }
    }
    for f in &mut funcs {
        // GEP bases are resolved before the mutation loop (the function is
        // borrowed mutably below), now with the alias-load entries.
        let bases = bases_with_alias_loads(f, &aliases);
        // Local allocas fed whole-object into a priority-read global via a
        // memcpy (the HAL `Init(&h)` idiom, epic-cc#463): a store into any
        // field of such an alloca needs the same rewrite as a direct store
        // into the global would, since the memcpy carries it there. No
        // field granularity here (the memcpy already flattens the struct),
        // so this is alloca-whole-object, not field-sensitive like the
        // direct-store case below.
        let mut alloca_feeds_lo: HashMap<String, Vec<String>> = HashMap::new();
        let mut alloca_feeds_hi: HashMap<String, Vec<String>> = HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                let Inst::Memcpy(mc) = inst else { continue };
                let Some((g, _)) = global_field_map(&ptr_of_val(&mc.dst), &bases) else {
                    continue;
                };
                let Some(src_reg) = ptr_of_val(&mc.src).strip_prefix('%').map(str::to_string)
                else {
                    continue;
                };
                if lo_read.iter().any(|(rg, _)| *rg == g) {
                    alloca_feeds_lo
                        .entry(src_reg.clone())
                        .or_default()
                        .push(g.clone());
                }
                if hi_read.iter().any(|(rg, _)| *rg == g) {
                    alloca_feeds_hi.entry(src_reg).or_default().push(g);
                }
            }
        }
        let in_lo = rewrite_lo.contains(&f.name);
        let in_hi = rewrite_hi.contains(&f.name);
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Store(s) = inst {
                    if let Some((g, sk)) = global_field_map(&s.ptr, &bases) {
                        // A handle feed is whole-object (the memcpy carries
                        // every field), so it matches regardless of `sk`.
                        let feeds_lo = lo_read
                            .iter()
                            .any(|(rg, rk)| *rg == g && (*rk == ALL_FIELDS || *rk == sk))
                            || handle_feeds_lo.contains(&g);
                        let feeds_hi = hi_read
                            .iter()
                            .any(|(rg, rk)| *rg == g && (*rk == ALL_FIELDS || *rk == sk))
                            || handle_feeds_hi.contains(&g);
                        if let Val::Global(fn_name) = &s.val {
                            let fn_name = fn_name.clone();
                            let lo_hit =
                                !in_lo && feeds_lo && shared_lo_set.contains(fn_name.as_str());
                            let hi_hit =
                                !in_hi && feeds_hi && shared_hi_set.contains(fn_name.as_str());
                            if lo_hit && hi_hit {
                                panic!(
                                    "legalize: store of @{fn_name} feeds both ISR priorities' read sets; no single copy serves both contexts"
                                );
                            }
                            let (spelling, prio) = if lo_hit {
                                stored_lo.insert(fn_name.clone());
                                (format!("{fn_name}{LO_SUFFIX}"), SpellPrio::Lo)
                            } else if hi_hit {
                                stored_hi.insert(fn_name.clone());
                                (format!("{fn_name}{HI_SUFFIX}"), SpellPrio::Hi)
                            } else {
                                (fn_name.clone(), SpellPrio::Main)
                            };
                            s.val = Val::Global(spelling.clone());
                            // Only defined functions are dispatchable
                            // callbacks: pointer stores whose value is
                            // another global (the `g_handle = &g_storage`
                            // idiom) are address plumbing, not a
                            // registrable spelling.
                            if defined.contains(fn_name.as_str()) {
                                let key = aliases
                                    .get(g.as_str())
                                    .cloned()
                                    .unwrap_or_else(|| g.clone());
                                spellings.entry(key).or_default().push((spelling, prio, sk));
                            }
                        }
                        continue;
                    }
                    let Some(root) = alloca_root_map(&s.ptr, &bases) else {
                        continue;
                    };
                    // The store's field inside the alloca object, summed
                    // over the GEP chain back to the root (mirrors
                    // alloca_field, which needs an immutable Func): the
                    // memcpy carries every field, so the store's field
                    // selects the spelling.
                    let fk = {
                        let mut cur = s.ptr.strip_prefix('%').map(str::to_string);
                        let mut k: u16 = 0;
                        let mut field = None;
                        while let Some(c) = cur {
                            if c == root {
                                field = Some(k);
                                break;
                            }
                            match bases.get(&c) {
                                Some((GepBase::Reg(r), gk, _)) => {
                                    k = k.wrapping_add(u16::from(*gk));
                                    cur = Some(r.clone());
                                }
                                _ => break,
                            }
                        }
                        field.unwrap_or(ALL_FIELDS)
                    };
                    if let Val::Global(fn_name) = &s.val {
                        let fn_name = fn_name.clone();
                        let lo_hit = !in_lo
                            && alloca_feeds_lo.contains_key(&root)
                            && shared_lo_set.contains(fn_name.as_str());
                        let hi_hit = !in_hi
                            && alloca_feeds_hi.contains_key(&root)
                            && shared_hi_set.contains(fn_name.as_str());
                        if lo_hit && hi_hit {
                            panic!(
                                "legalize: store of @{fn_name} (via a memcpy'd local) feeds both ISR priorities' read sets; no single copy serves both contexts"
                            );
                        }
                        let (spelling, prio) = if lo_hit {
                            stored_lo.insert(fn_name.clone());
                            (format!("{fn_name}{LO_SUFFIX}"), SpellPrio::Lo)
                        } else if hi_hit {
                            stored_hi.insert(fn_name.clone());
                            (format!("{fn_name}{HI_SUFFIX}"), SpellPrio::Hi)
                        } else {
                            (fn_name.clone(), SpellPrio::Main)
                        };
                        s.val = Val::Global(spelling.clone());
                        // The alloca feeds one storage per priority side;
                        // each of those storages now holds this spelling.
                        let storages = match prio {
                            SpellPrio::Lo => alloca_feeds_lo.get(&root).cloned(),
                            SpellPrio::Hi => alloca_feeds_hi.get(&root).cloned(),
                            // A main-band spelling is occupiable at every
                            // site, so it is recorded under every storage
                            // the alloca feeds, like the direct-store arm.
                            SpellPrio::Main => {
                                let mut fed: Vec<String> =
                                    alloca_feeds_lo.get(&root).cloned().unwrap_or_default();
                                fed.extend(alloca_feeds_hi.get(&root).cloned().unwrap_or_default());
                                Some(fed)
                            }
                        };
                        if let Some(storages) = storages {
                            for storage in storages {
                                // Only defined functions are dispatchable
                                // callbacks (see the direct-store case).
                                if !defined.contains(fn_name.as_str()) {
                                    continue;
                                }
                                let key = aliases
                                    .get(storage.as_str())
                                    .cloned()
                                    .unwrap_or_else(|| storage.clone());
                                spellings.entry(key).or_default().push((
                                    spelling.clone(),
                                    prio,
                                    fk,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    // Param-forwarded call arguments (epic-cc#137), per priority: a call
    // passing a named function into a param that a callee stores into a
    // priority-read global (the `EPIC_GPIO_RegisterChangeCallback(on_rb_change)`
    // shape) must pass that priority's copy, or the ISR loads the
    // original's address. The rewrite lands at the call site, outside
    // the rewritten priority's context. A dual-feeding argument panics
    // as above.
    for f in &mut funcs {
        let in_lo = rewrite_lo.contains(&f.name);
        let in_hi = rewrite_hi.contains(&f.name);
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Call(c) = inst {
                    // Every param of the callee that feeds a priority-read
                    // global must be rewritten, not just the first match
                    // (a callee may store two params into two globals).
                    // Directions are collected before mutating so a
                    // dual-feeding argument panics instead of landing
                    // half-rewritten.
                    let mut lo_args: Vec<usize> = Vec::new();
                    let mut hi_args: Vec<usize> = Vec::new();
                    if !in_lo {
                        for (_, pi, _) in
                            lo_params.iter().filter(|(callee, _, _)| callee == &c.func)
                        {
                            if let Some(a) = c.args.get(*pi) {
                                if let Val::Global(fn_name) = &a.val {
                                    if shared_lo_set.contains(fn_name.as_str()) {
                                        lo_args.push(*pi);
                                    }
                                }
                            }
                        }
                    }
                    if !in_hi {
                        for (_, pi, _) in
                            hi_params.iter().filter(|(callee, _, _)| callee == &c.func)
                        {
                            if let Some(a) = c.args.get(*pi) {
                                if let Val::Global(fn_name) = &a.val {
                                    if shared_hi_set.contains(fn_name.as_str()) {
                                        hi_args.push(*pi);
                                    }
                                }
                            }
                        }
                    }
                    for pi in &lo_args {
                        if hi_args.contains(pi) {
                            panic!(
                                "legalize: call argument feeds both ISR priorities' read sets; no single copy serves both contexts"
                            );
                        }
                    }
                    for pi in lo_args {
                        let fn_name = match c.args.get(pi).map(|a| &a.val) {
                            Some(Val::Global(g)) => g.clone(),
                            _ => continue,
                        };
                        let storages: Vec<String> = lo_params
                            .iter()
                            .filter(|(callee, p, _)| callee == &c.func && *p == pi)
                            .map(|(_, _, s)| s.clone())
                            .collect();
                        for storage in &storages {
                            let key = aliases
                                .get(storage.as_str())
                                .cloned()
                                .unwrap_or_else(|| storage.clone());
                            spellings.entry(key).or_default().push((
                                format!("{fn_name}{LO_SUFFIX}"),
                                SpellPrio::Lo,
                                ALL_FIELDS,
                            ));
                        }
                        stored_lo.insert(fn_name.clone());
                        if let Some(a) = c.args.get_mut(pi) {
                            a.val = Val::Global(format!("{fn_name}{LO_SUFFIX}"));
                        }
                    }
                    for pi in hi_args {
                        let fn_name = match c.args.get(pi).map(|a| &a.val) {
                            Some(Val::Global(g)) => g.clone(),
                            _ => continue,
                        };
                        let storages: Vec<String> = hi_params
                            .iter()
                            .filter(|(callee, p, _)| callee == &c.func && *p == pi)
                            .map(|(_, _, s)| s.clone())
                            .collect();
                        for storage in &storages {
                            let key = aliases
                                .get(storage.as_str())
                                .cloned()
                                .unwrap_or_else(|| storage.clone());
                            spellings.entry(key).or_default().push((
                                format!("{fn_name}{HI_SUFFIX}"),
                                SpellPrio::Hi,
                                ALL_FIELDS,
                            ));
                        }
                        stored_hi.insert(fn_name.clone());
                        if let Some(a) = c.args.get_mut(pi) {
                            a.val = Val::Global(format!("{fn_name}{HI_SUFFIX}"));
                        }
                    }
                }
            }
        }
    }
    // Spelling forwarding across the whole-object memcpys (the
    // `Init(&h)` idiom's second hop): spellings recorded under a handle
    // or alloca source also land in the ISR-read storage the memcpy
    // copies into, with the field preserved. Without this the dispatch
    // site loading that storage scopes to an empty list and falls back.
    {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for sf in &funcs {
            let bases = bases_with_alias_loads(sf, &aliases);
            for b in &sf.blocks {
                for inst in &b.insts {
                    let Inst::Memcpy(mc) = inst else { continue };
                    let Some((dst, _)) = global_field_map(&ptr_of_val(&mc.dst), &bases) else {
                        continue;
                    };
                    let dst_key = aliases.get(dst.as_str()).cloned().unwrap_or(dst);
                    let src_ptr = ptr_of_val(&mc.src);
                    if let Some((src_g, _)) = global_field_map(&src_ptr, &bases) {
                        let src_key = aliases.get(src_g.as_str()).cloned().unwrap_or(src_g);
                        if src_key != dst_key {
                            pairs.push((dst_key, src_key));
                        }
                    } else if let Some(src_reg) = src_ptr.strip_prefix('%') {
                        if let Some(pi) = sf.params.iter().position(|p| p.name == src_reg) {
                            for handle in globals_passed_as_param(&funcs, &sf.name, pi) {
                                let src_key =
                                    aliases.get(handle.as_str()).cloned().unwrap_or(handle);
                                if src_key != dst_key {
                                    pairs.push((dst_key.clone(), src_key));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Chained handle copies converge: repeat until no list grows.
        loop {
            let mut grew = false;
            for (dst, src) in &pairs {
                let entries = spellings.get(src).cloned().unwrap_or_default();
                for (name, prio, field) in entries {
                    let slot = spellings.entry(dst.clone()).or_default();
                    if !slot
                        .iter()
                        .any(|(n, p, f)| n == &name && p == &prio && f == &field)
                    {
                        slot.push((name, prio, field));
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }
    (
        Module {
            globals: m.globals,
            funcs,
            module_asm: m.module_asm,
        },
        stored_lo,
        stored_hi,
        spellings,
    )
}

/// Rewrite every `Val::Global(f)` in `inst` to `Val::Global(f+suffix)`
/// when `f` is in `shared` (a function duplicated for an ISR context). Covers every
/// inst variant that carries a `Val`; the pointer-returning `sink_ptr_select`
/// bodies and the `Call.func` target are handled separately.
fn rewrite_inst_vals(
    inst: &mut Inst,
    shared: &HashSet<&str>,
    suffix: &str,
    stored: &mut HashSet<String>,
) {
    fn rv(v: &mut Val, shared: &HashSet<&str>, suffix: &str, stored: &mut HashSet<String>) {
        if let Val::Global(g) = v {
            if shared.contains(g.as_str()) {
                stored.insert(g.clone());
                *v = Val::Global(format!("{g}{suffix}"));
            }
        }
    }
    match inst {
        Inst::Store(s) => rv(&mut s.val, shared, suffix, stored),
        Inst::Bin(b) => {
            rv(&mut b.a, shared, suffix, stored);
            rv(&mut b.b, shared, suffix, stored);
        }
        Inst::Ret(Some((_, v)), _) => rv(v, shared, suffix, stored),
        Inst::Zext(z) => rv(&mut z.val, shared, suffix, stored),
        Inst::Sext(x) => rv(&mut x.val, shared, suffix, stored),
        Inst::Trunc(t) => rv(&mut t.val, shared, suffix, stored),
        Inst::IntToPtr(p) => rv(&mut p.val, shared, suffix, stored),
        Inst::Icmp(i) => {
            rv(&mut i.a, shared, suffix, stored);
            rv(&mut i.b, shared, suffix, stored);
        }
        Inst::Select(s) => {
            rv(&mut s.cond, shared, suffix, stored);
            rv(&mut s.a, shared, suffix, stored);
            rv(&mut s.b, shared, suffix, stored);
        }
        Inst::Call(c) => {
            for arg in &mut c.args {
                rv(&mut arg.val, shared, suffix, stored);
            }
        }
        Inst::Phi(p) => {
            for (v, _) in &mut p.incoming {
                rv(v, shared, suffix, stored);
            }
        }
        Inst::Memcpy(mc) => {
            rv(&mut mc.dst, shared, suffix, stored);
            rv(&mut mc.src, shared, suffix, stored);
            if let MemLen::Reg(v) = &mut mc.len {
                rv(v, shared, suffix, stored);
            }
        }
        Inst::Freeze(fr) => rv(&mut fr.val, shared, suffix, stored),
        Inst::FloatBin(fb) => {
            rv(&mut fb.a, shared, suffix, stored);
            rv(&mut fb.b, shared, suffix, stored);
        }
        Inst::Fcmp(fc) => {
            rv(&mut fc.a, shared, suffix, stored);
            rv(&mut fc.b, shared, suffix, stored);
        }
        Inst::FloatConv(fc) => rv(&mut fc.val, shared, suffix, stored),
        _ => {}
    }
}

/// Fills the `callees` candidate list of every indirect call site. The
/// candidate set is the whole-program address-taken set (every function whose
/// address appears as a value), split by call-graph context so an ISR-context
/// site references only `_isr` copies and a main-context site only the
/// originals: the overlay allocator's disjoint-region analysis depends on it.
/// The exception, derived from the same decision as the store rewrite: when
/// the rewrite stored a priority's copy into a shared storage global, every
/// other context reading that storage now holds the copy's address at
/// runtime, so its dispatch sites must list the copy, not the vanished
/// original (epic-cc#568).
/// `!callees` metadata stays unconsumed (clang omits it for table loads).
fn fill_indirect_callees(
    m: &mut Module,
    stored_lo: &HashSet<String>,
    stored_hi: &HashSet<String>,
    spellings: &HashMap<String, Vec<(String, SpellPrio, u16)>>,
) {
    // A candidate is a rewritten copy of a stored original: `f_isr` with
    // `f` in `stored_lo` (symmetrically for the high suffix).
    let stored_copy = |g: &str, stored: &HashSet<String>, suffix: &str| {
        g.strip_suffix(suffix)
            .is_some_and(|orig| stored.contains(orig))
    };
    // Address-taken set: every `Val::Global(f)` where `f` is a defined
    // function. Non-const globals with `ptr` initializers are zeroinit and
    // contribute nothing; const fp tables panic at parse: outside this scope.
    let defined: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    let mut addr_taken: HashSet<String> = HashSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                collect_global_vals(inst, &mut addr_taken);
            }
        }
    }
    // A const struct's function-pointer fields hold function addresses in
    // flash: the functions are address-taken, so an indirect call through
    // a loaded field can dispatch them (epic-cc#154).
    for g in &m.globals {
        for (_, f) in &g.refs {
            addr_taken.insert(f.clone());
        }
    }
    // Arity and width maps for the candidate filter: an indirect call
    // site invokes only a candidate with the matching argument count
    // and matching widths. Without the arity check, a 1-arg ISR
    // callback site collects 0-arg callbacks and isel panics with no
    // lowering for the mismatch; without the width check, an i8 arg site
    // collects ptr-param tasks and isel panics the same way (epic-cc#152).
    // An `_isr` copy shares its original's params, so both checks carry over.
    let arity: HashMap<String, usize> = m
        .funcs
        .iter()
        .map(|f| (f.name.clone(), f.params.len()))
        .collect();
    let param_widths: HashMap<String, Vec<u16>> = m
        .funcs
        .iter()
        .map(|f| {
            (
                f.name.clone(),
                f.params.iter().map(|p| u16::from(p.width)).collect(),
            )
        })
        .collect();

    // Priority-partitioned reachability over the post-duplication module:
    // the ISR roots keep their priorities (copies have `isr` cleared),
    // and the rewritten call edges already point each context at its
    // own copies, so reachability from each root set is exactly the set
    // of functions that may execute in that context. Each side runs the
    // store-edge fixpoint (a stored callback joins through the global
    // the ISR reads, not through a direct call).
    let (adj2, _) = isr_adjacency(m);
    // Storage keys are canonicalized through the pointer aliases so a
    // site loading through `g_handle = &g_storage` scopes to the same
    // spellings a direct store into `g_storage` recorded.
    let aliases = global_ptr_aliases(m);
    let lo_roots: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority != 1)
        .map(|f| f.name.as_str())
        .collect();
    let hi_roots: HashSet<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr && f.irq_priority == 1)
        .map(|f| f.name.as_str())
        .collect();
    let (lo2, _, _) = isr_context_for(m, &lo_roots, &adj2, &defined);
    let (hi2, _, _) = isr_context_for(m, &hi_roots, &adj2, &defined);
    let main2: HashSet<String> = reachable(&["main"], &adj2);

    for f in &mut m.funcs {
        // The contexts this site may execute in: the copies created
        // above run in exactly one priority each, while a shared
        // original (reachable but rewritten away from both ISR
        // contexts) runs only in main.
        let in_lo = lo2.contains(&f.name);
        let in_hi = hi2.contains(&f.name);
        let in_main = main2.contains(&f.name);
        // Per-site dispatch storages: the global field each indirect
        // pointer is loaded from (ADR-038). Resolvable sites scope their
        // candidates to that storage's spellings; unresolvable sites
        // keep the context-scoped whole-program list.
        let bases = bases_with_alias_loads(f, &aliases);
        let load_storages: HashMap<String, (String, u16)> = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Load(l) => global_field_map(&l.ptr, &bases).map(|(g, fk)| {
                    (
                        l.dst.clone(),
                        (aliases.get(g.as_str()).cloned().unwrap_or(g), fk),
                    )
                }),
                _ => None,
            })
            .collect();
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Call(c) = inst {
                    // A direct call's `func` is a defined function name; an
                    // indirect call's `func` is the SSA register (numeric),
                    // not a defined function. Only the latter gets candidates.
                    if defined.contains(c.func.as_str()) {
                        continue;
                    }
                    // The spellings this site's storage holds for the field
                    // the site loads: a site loading an unregistered field
                    // degrades to the legacy list, since no tracked spelling
                    // claims that value (epic-cc#467's USART RX shape).
                    let scoped = load_storages.get(&c.func).and_then(|(s, fk)| {
                        spellings.get(s).map(|entries| {
                            entries
                                .iter()
                                .filter(|(_, _, f)| *f == ALL_FIELDS || *f == *fk)
                                .collect::<Vec<_>>()
                        })
                    });
                    let mut cands: Vec<String> = match &scoped {
                        Some(entries) if !entries.is_empty() => entries
                            .iter()
                            .filter(|(_, prio, _)| match prio {
                                // A site must not execute a frame from the
                                // priority that can preempt it: the higher
                                // context re-enters the lower one's static
                                // frame mid-callback (ADR-038). A site with
                                // no execution context (an uncalled
                                // original) admits everything.
                                SpellPrio::Main => true,
                                SpellPrio::Lo => !in_hi,
                                SpellPrio::Hi => !in_lo,
                            })
                            .map(|(n, _, _)| n.clone())
                            .collect(),
                        _ => addr_taken
                            .iter()
                            .filter(|g| {
                                (!in_main
                                    || (!lo2.contains(*g) && !hi2.contains(*g))
                                    || stored_copy(g, stored_lo, LO_SUFFIX)
                                    || stored_copy(g, stored_hi, HI_SUFFIX))
                                    && (!in_lo
                                        || lo2.contains(*g)
                                        || stored_copy(g, stored_hi, HI_SUFFIX))
                                    && (!in_hi
                                        || hi2.contains(*g)
                                        || stored_copy(g, stored_lo, LO_SUFFIX))
                            })
                            .cloned()
                            .collect(),
                    };
                    cands.retain(|g| arity.get(g).copied() == Some(c.args.len()));
                    cands.retain(|g| {
                        let widths = param_widths.get(g).map(Vec::as_slice).unwrap_or(&[]);
                        c.args
                            .iter()
                            .zip(widths.iter())
                            .all(|(a, &w)| u16::from(a.ty.map(|t| t.bytes()).unwrap_or(2)) == w)
                    });
                    // ADR-038: a priority site whose dispatch storage field
                    // holds only the other priority's spelling cannot
                    // dispatch soundly (the higher context re-enters the
                    // lower's static frame). Fail loudly instead of skipping
                    // the call (epic-cc#467's silent-skip class). Checked
                    // after the arity and width retains so the guarantee
                    // covers the final list.
                    if (in_lo || in_hi)
                        && scoped.as_ref().map_or(false, |e| !e.is_empty())
                        && cands.is_empty()
                    {
                        let (s, _) = &load_storages[&c.func];
                        panic!(
                            "legalize: dispatch site in @{} reads @{} whose stored callback serves only the other priority; register per-priority spellings or confine the dispatch (ADR-038)",
                            f.name,
                            s
                        );
                    }
                    // A candidate that is a duplicated ORIGINAL
                    // (address-taken elsewhere, e.g. a select arm) must
                    // dispatch the copy for each context this site
                    // executes in: no context can run a foreign frame
                    // (ADR-013). A site live in both ISR contexts whose
                    // candidate has both copies is unrepresentable:
                    // panic loudly.
                    for g in &mut cands {
                        let mut dispatched: Option<String> = None;
                        for (executes, suffix) in [(in_lo, LO_SUFFIX), (in_hi, HI_SUFFIX)] {
                            if !executes {
                                continue;
                            }
                            let copy = format!("{g}{suffix}");
                            if defined.contains(&copy) {
                                if let Some(prev) = &dispatched {
                                    if *prev != copy {
                                        panic!(
                                            "legalize: indirect call candidate @{g} needs both ISR priorities' copies; no single dispatch serves both contexts"
                                        );
                                    }
                                } else {
                                    dispatched = Some(copy);
                                }
                            }
                        }
                        if let Some(copy) = dispatched {
                            *g = copy;
                        }
                    }
                    cands.sort();
                    cands.dedup();
                    // Self-check: every candidate must be a defined function.
                    for g in &cands {
                        assert!(
                            defined.contains(g.as_str()),
                            "legalize: indirect call candidate @{g} is not a defined function"
                        );
                    }
                    c.callees = cands;
                }
            }
        }
    }
}

/// Collect every `Val::Global` in `inst` into `out` (the address-taken set).
fn collect_global_vals(inst: &Inst, out: &mut HashSet<String>) {
    fn push(v: &Val, out: &mut HashSet<String>) {
        if let Val::Global(g) = v {
            out.insert(g.clone());
        }
    }
    match inst {
        Inst::Store(s) => push(&s.val, out),
        Inst::Bin(b) => {
            push(&b.a, out);
            push(&b.b, out);
        }
        Inst::Ret(Some((_, v)), _) => push(v, out),
        Inst::Zext(z) => push(&z.val, out),
        Inst::Sext(x) => push(&x.val, out),
        Inst::Trunc(t) => push(&t.val, out),
        Inst::IntToPtr(p) => push(&p.val, out),
        Inst::Icmp(i) => {
            push(&i.a, out);
            push(&i.b, out);
        }
        Inst::Select(s) => {
            push(&s.cond, out);
            push(&s.a, out);
            push(&s.b, out);
        }
        Inst::Call(c) => {
            for arg in &c.args {
                push(&arg.val, out);
            }
        }
        Inst::Phi(p) => {
            for (v, _) in &p.incoming {
                push(v, out);
            }
        }
        Inst::Memcpy(mc) => {
            push(&mc.dst, out);
            push(&mc.src, out);
            if let MemLen::Reg(v) = &mc.len {
                push(v, out);
            }
        }
        Inst::Freeze(fr) => push(&fr.val, out),
        Inst::FloatBin(fb) => {
            push(&fb.a, out);
            push(&fb.b, out);
        }
        Inst::Fcmp(fc) => {
            push(&fc.a, out);
            push(&fc.b, out);
        }
        Inst::FloatConv(fc) => push(&fc.val, out),
        _ => {}
    }
}

/// The runtime routine for a scalar binop, or `None` when legalize leaves the
/// op as a `Bin` (add/sub/and/or/xor, and i1 forms clang omits).
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

/// Rewrites one `Inst::Bin` into the runtime call, recording the routine as
/// used. Returns `None` when the binop stays as-is: non-lowered ops, and
/// const-count shifts (isel inlines those: the count arrives as a `Const`).
/// Folds a `Bin` with both operands `Val::Const` into an `Inst::Freeze`
/// carrying the literal result. isel has no path for a const-const shape
/// (clang folds these upstream, so only hand-written IR reaches here with
/// both sides constant). Several ops panic there with no lowering, and `sub`
/// miscompiles by reading the second constant as a file address. `Freeze`
/// copies a `Val::Const` into `dst`'s slot via a plain `MOVLW`.
///
/// Returns `None` (leave the `Bin` unfolded) when either operand isn't
/// constant, or when folding would have to invent a result for something
/// that's already defined as LLVM poison: division/remainder by a
/// zero constant (the runtime routine already documents that behavior) or a
/// shift count outside `[0, width)` (isel's existing assert is the poison
/// check).
fn fold_const_bin(b: &ir::Bin) -> Option<Inst> {
    // i1 is icmp/fcmp's output type only; isel asserts against a Bin typed
    // i1 (`b.ty != Ty::I1`, arithmetic bit-widths make no sense on a 1-bit
    // value). Ty::bytes() maps I1 to 1 byte like I8, so folding it here
    // would manufacture an out-of-range "i1" constant instead of hitting
    // that guard. Leaves it unfolded so isel's existing check still fires.
    if b.ty == Ty::I1 {
        return None;
    }
    let (Val::Const(a), Val::Const(k)) = (&b.a, &b.b) else {
        return None;
    };
    let width = u32::from(b.ty.bytes()) * 8;
    let result = eval_binop(b.op, width, *a, *k)?;
    Some(Inst::Freeze(ir::Freeze {
        dst: b.dst.clone(),
        ty: b.ty,
        val: Val::Const(result),
        loc: None,
    }))
}

/// Same fold for `Icmp`; the result is always `i1`.
fn fold_const_icmp(c: &Icmp) -> Option<Inst> {
    let (Val::Const(a), Val::Const(k)) = (&c.a, &c.b) else {
        return None;
    };
    let width = u32::from(c.ty.bytes()) * 8;
    let result = eval_icmp(&c.pred, width, *a, *k);
    Some(Inst::Freeze(ir::Freeze {
        dst: c.dst.clone(),
        ty: Ty::I1,
        val: Val::Const(i64::from(result)),
        loc: None,
    }))
}

fn const_mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// Interpret the low `width` bits of `v` as a two's-complement signed value.
fn sign_extend(v: u64, width: u32) -> i64 {
    let shift = 64 - width;
    ((v << shift) as i64) >> shift
}

/// Canonicalize a raw `width`-bit result as the unsigned form (`0..2^width`),
/// matching how a plain arithmetic/bitwise/shift op's result reads when the
/// operation itself carries no sign (only `sdiv`/`srem`/`ashr` do).
fn canon_unsigned(v: u64, width: u32) -> i64 {
    (v & const_mask(width)) as i64
}

/// Canonicalize a raw `width`-bit result as its signed form, for the ops
/// (`sdiv`/`srem`/`ashr`) whose whole point is a signed interpretation.
fn canon_signed(v: u64, width: u32) -> i64 {
    sign_extend(v & const_mask(width), width)
}

/// Evaluates a binop on two constants, masked/interpreted at `width` bits to
/// match isel's own per-byte truncation convention (`(k >> idx*8) & 0xFF`).
/// The result re-masks to `width` bits too: the IR text carries no type
/// tag on a bare constant, so a folded `add i8 200, 100` reads as `44`,
/// not the unmasked `300`, the result the width implies.
fn eval_binop(op: BinOp, width: u32, a: i64, b: i64) -> Option<i64> {
    let m = const_mask(width);
    let au = (a as u64) & m;
    let bu = (b as u64) & m;
    Some(match op {
        BinOp::Add => canon_unsigned(au.wrapping_add(bu), width),
        BinOp::Sub => canon_unsigned(au.wrapping_sub(bu), width),
        BinOp::And => canon_unsigned(au & bu, width),
        BinOp::Or => canon_unsigned(au | bu, width),
        BinOp::Xor => canon_unsigned(au ^ bu, width),
        BinOp::Mul => canon_unsigned(au.wrapping_mul(bu), width),
        BinOp::UDiv => {
            if bu == 0 {
                return None;
            };
            canon_unsigned(au / bu, width)
        }
        BinOp::URem => {
            if bu == 0 {
                return None;
            };
            canon_unsigned(au % bu, width)
        }
        BinOp::SDiv => {
            if bu == 0 {
                return None;
            };
            let q = sign_extend(au, width).wrapping_div(sign_extend(bu, width));
            canon_signed(q as u64, width)
        }
        BinOp::SRem => {
            if bu == 0 {
                return None;
            };
            let r = sign_extend(au, width).wrapping_rem(sign_extend(bu, width));
            canon_signed(r as u64, width)
        }
        BinOp::Shl | BinOp::LShr | BinOp::AShr => {
            if !(0..i64::from(width)).contains(&b) {
                return None;
            };
            let shift = b as u32;
            match op {
                BinOp::Shl => canon_unsigned(au.wrapping_shl(shift), width),
                BinOp::LShr => canon_unsigned(au >> shift, width),
                BinOp::AShr => canon_signed((sign_extend(au, width) >> shift) as u64, width),
                _ => unreachable!(),
            }
        }
    })
}

fn eval_icmp(pred: &str, width: u32, a: i64, b: i64) -> bool {
    let m = const_mask(width);
    let au = (a as u64) & m;
    let bu = (b as u64) & m;
    match pred {
        "eq" => au == bu,
        "ne" => au != bu,
        "ult" => au < bu,
        "ule" => au <= bu,
        "ugt" => au > bu,
        "uge" => au >= bu,
        "slt" => sign_extend(au, width) < sign_extend(bu, width),
        "sle" => sign_extend(au, width) <= sign_extend(bu, width),
        "sgt" => sign_extend(au, width) > sign_extend(bu, width),
        "sge" => sign_extend(au, width) >= sign_extend(bu, width),
        // ir::parse validates the predicate against this exact 10-entry set
        // before an Icmp can exist, so anything else here is unreachable.
        other => unreachable!("legalize: unknown icmp predicate {other}"),
    }
}

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
            CallArg {
                ty: Some(b.ty),
                val: b.a.clone(),
                byval: None,
                sret: false,
            },
            CallArg {
                ty: Some(b.ty),
                val: b.b.clone(),
                byval: None,
                sret: false,
            },
        ],
        callees: Vec::new(),
        loc: None,
    }))
}

fn param(name: &str, width: u8) -> Param {
    Param {
        name: name.into(),
        width,
        byval: None,
        sret: false,
        ptr: false,
    }
}

/// The injected runtime routine definitions. Each is an ordinary function
/// with one empty block containing only the scratch alloca, so `alloc`
/// places the frame and the recipe emitters resolve every slot
/// address from the map (`{func}::{param}`, `{func}::__scr`).
///
/// # The scratch layout contract (sizes + offsets)
///
/// These byte offsets form the cross-emitter contract: the injection step
/// provides the buffers, the mul/div/rem emission reads them, then the shift
/// emission reads them. The recipes read their inputs from the param
/// slots (`a`/`b`, `num`/`den`, `val`/`cnt`), write the result to the retval
/// slots, and use `__scr` strictly by offset. On PIC14 every routine's frame
/// stays inside ONE GPR bank, because the recipes' loops are skip-sensitive:
/// no BANKSEL sits between a test and its target or inside a carry idiom.
/// PIC18 needs the same guarantee for a narrower set of routines: the i16/i32
/// signed wrappers, the float bodies and the conversions test a frame byte
/// and skip an instruction that names another. Its bank is the
/// 256-byte `BSR` bank rather than a `ram_banks` region. `alloc`
/// (`routine_base`) rounds a routine's base accordingly; `isel` verifies the
/// placement (epic-cc#6, epic-cc#509).
///
/// | routine | `__scr` size | offsets |
/// |---|---|---|
/// | `__mul_u8` | 6 | `bk`@0 (multiplier backup, shifted to test bits), `cnt`@1 (loop counter, 8), `r_lo`@2 / `r_hi`@3 (16-bit running product), `t_lo`@4 / `t_hi`@5 (shifted multiplicand) |
/// | `__mul_u16` | 14 | `bk_lo`@0 / `bk_hi`@1 (multiplier backup), `cnt`@2 (loop counter, 16), `r`@3-6 (32-bit running product), `t`@7-10 (shifted multiplicand), `spare`@11-13 (recipe scratch) |
/// | `__udiv_u8`, `__urem_u8` | 4 | `rem_lo`@0 / `rem_hi`@1 (partial remainder: 2 bytes, since the 8-bit rem shift can carry), `cnt`@2 (loop counter, 8), `restore`@3 (restore-step scratch) |
/// | `__udiv_u16`, `__urem_u16` | 7 | `rem`@0-1 (partial remainder), `cnt`@2 (loop counter, 16), `spare`@3 (recipe scratch), `restore`@4-6 (restore-step scratch) |
/// | `__sdiv_i8`, `__srem_i8` | 5 | `flags`@0 (sign state: bit0 = negate quotient, bit1 = negate remainder; `\|num\|`/`\|den\|` live in the param slots), `rem_lo`@1 / `rem_hi`@2, `cnt`@3, `restore`@4 |
/// | `__sdiv_i16`, `__srem_i16` | 7 | `flags`@0 (as i8), `rem`@1-2, `cnt`@3, `restore`@4-5, `spare`@6 |
/// | `__shl_u8`, `__lshr_u8`, `__ashr_i8` | 3 | `cnt`@0 (masked count / loop counter: the value shifts in the `val` param slot), `spare`@1-2 (recipe scratch) |
/// | `__shl_u16`, `__lshr_u16`, `__ashr_i16` | 4 | `cnt`@0-1 (masked count / loop counter), `spare`@2-3 (recipe scratch) |
/// | `__mul_u32` | 11 | `bk_lo`@0 / `bk_hi`@1 (multiplier backup: 2 bytes, the low 16 bits first, reloaded from `b`'s high half for the second 16 of the 32 iterations), `cnt`@2 (loop counter, 32), `r`@3-6 (32-bit running product: the low 32 bits of the full product), `t`@7-10 (shifted multiplicand: 4 bytes, shifting left with wraparound, so the shifted-out high bits drop and i32 `mul` wraps) |
/// | `__udiv_u32`, `__urem_u32` | 10 | `rem`@0-3 (partial remainder: full 32 bits, with no carry out for a 32/32 divide), `den`@4-7 (denominator copy: the divmod subtracts/restores against this, so the param slot stays untouched), `cnt`@8 (loop counter, 32), `spare`@9 (recipe scratch) |
/// | `__sdiv_i32`, `__srem_i32` | 12 | the divmod part at the unsigned offsets: `rem`@0-3, `den`@4-7, `cnt`@8, `spare`@9, plus `flags`@10 (sign state: bit0 = negate quotient = num<0 XOR den<0, bit1 = negate remainder = num<0), `spare`@11 |
/// | `__shl_u32`, `__lshr_u32`, `__ashr_i32` | 2 | `cnt`@0 (masked count / loop counter: the value shifts in the `val` param slot), `spare`@1 (recipe scratch) |
/// | `__add_f32`, `__sub_f32` | 14 | `sa`@0 (sign of a), `ea`@1 (biased exponent of a), `ma`@2-4 (24-bit mantissa of a with the implicit bit), `sb`@5, `eb`@6, `mb`@7-9 (same for b), `stick`@10 (sticky collector for the right-alignment shift), `cnt`@11 (alignment/normalize shift counter), `ta1`@12 / `ta2`@13 (the 24-bit fraction window; `ta0` reuses the dead `eb` slot at offset 6) |
/// | `__mul_f32` | 14 | `sign`@0 (result sign = sa XOR sb), `e`@1-2 (biased result exponent: e1+e2-127, 16-bit intermediate), `bk`@3-5 (multiplier backup, shifted to test bits), `cnt`@6 (loop counter, 24), `m`@7-10 (running product: the top 25 bits of the 24x24 product accumulate here), `spare`@11-13 (rounding scratch) |
/// | `__div_f32` | 12 | `sign`@0 (result sign = sa XOR sb), `e`@1-2 (biased result exponent: e1-e2+127, 16-bit intermediate), `rem`@3-6 (partial remainder: 4 bytes, since the 24-bit rem shift can carry a bit), `den`@7-9 (denominator copy: the restoring subtract/restore reads this, the param slot stays untouched), `cnt`@10 (loop counter, 24), `spare`@11 (rounding scratch) |
/// | `__cmp_f32` | 6 | `tmp`@0-1 (byte-compare scratch), `flags`@2 (sign-state / NaN-check flags), `spare`@3-5 |
/// | `__uitofp_f32`, `__sitofp_f32` | 8 | `cnt`@0 (leading-1 shift counter), `e`@1-2 (biased result exponent: 127+31-shifts), `guard`@3 (the round/guard bit), `stick`@4 (sticky), `spare`@5-7 |
/// | `__fptoui_f32`, `__fptosi_f32` | 8 | `e`@0 (biased exponent), `cnt`@1 (right-shift count: 127-e+23), `m`@2-4 (mantissa working copy, shifted right in place), `sign`@5 (fptosi only), `spare`@6-7 |
///
/// Notes: div-by-zero is LLVM poison: the loop runs (den = 0 gives quotient
/// 0xFFFF, remainder 0), any value is legal, no guard. Variable-shift counts
/// arrive unmasked and mask to `width - 1` inside the routine. The
/// signed wrappers abs in place in the param slots (unsigned abs, so INT_MIN
/// is safe), run the unsigned divmod, then negate per the flags byte. The
/// soft-float routines take their operands in 4-byte slots (`a`/`b` are the
/// f32 bytes; `val` is the 4-byte int slot: an i8/i16 source or result uses
/// the low bytes), write the result to the retval slots, and use `__scr`
/// strictly by offset (the recipe contract).
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
        "__shl_u8" | "__lshr_u8" | "__ashr_i8" => {
            (Ty::I8, vec![param("val", 1), param("cnt", 1)], 3)
        }
        "__shl_u16" | "__lshr_u16" | "__ashr_i16" => {
            (Ty::I16, vec![param("val", 2), param("cnt", 2)], 4)
        }
        "__shl_u32" | "__lshr_u32" | "__ashr_i32" => {
            (Ty::I32, vec![param("val", 4), param("cnt", 4)], 2)
        }
        // The soft-float routines (f32 slots are 4 bytes).
        "__add_f32" | "__sub_f32" | "__mul_f32" => {
            (Ty::F32, vec![param("a", 4), param("b", 4)], 14)
        }
        "__div_f32" => (Ty::F32, vec![param("a", 4), param("b", 4)], 12),
        "__cmp_f32" => (Ty::I8, vec![param("a", 4), param("b", 4)], 6),
        "__uitofp_f32" | "__sitofp_f32" => (Ty::F32, vec![param("val", 4)], 8),
        "__fptoui_f32" | "__fptosi_f32" => (Ty::I32, vec![param("val", 4)], 8),
        other => panic!("legalize: unknown runtime routine {other}"),
    };
    Func {
        name: name.into(),
        ret: Some(ret),
        params,
        blocks: vec![Block {
            label: "entry".into(),
            insts: vec![Inst::Alloca(Alloca {
                dst: "__scr".into(),
                size: scr,
                loc: None,
            })],
        }],
        isr: false, // runtime routines stay outside the interrupt context
        irq_priority: 0,
        naked: false,
        variadic: false,
    }
}
