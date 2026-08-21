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
//! - Every f32 op (Milestone 15) lowers to a soft-float runtime call:
//!   `fadd`/`fsub`/`fmul`/`fdiv` → `__add_f32`/`__sub_f32`/`__mul_f32`/
//!   `__div_f32` (dst/ty preserved, both operands copied as float args);
//!   `fcmp <pred>` → `%c = call i8 @__cmp_f32(a, b)` + the per-predicate
//!   icmp/select materialization tree over the tri-state byte
//!   (0 = equal, 1 = a < b, 2 = a > b, 3 = unordered) — an OR predicate
//!   is `select i1 <c==k1>, i1 true, i1 <c==k2>`, never an i1 binop (isel
//!   rejects those); `fptosi`/`fptoui`/`sitofp`/`uitofp` → the four
//!   conversion routines; `fpext`/`fptrunc` (f32→f32 — double == float on
//!   msp430) → a plain `freeze` copy, no call.
//!
//! The used routine `Func`s are then injected into the module: ordinary
//! functions (name/ret/params per the ABI table below) with one empty block
//! holding only the scratch alloca, so `alloc` sizes the routine frame and
//! Tasks 3/4's recipe emitters read their working state from
//! `{func}::__scr` + offset. Only the routines actually used are injected
//! (cleaner text artifacts).
//!
//! A routine both the main and the interrupt context reach is injected
//! TWICE — `__mul_u8` and `__mul_u8_isr` — so the two contexts never share
//! one frame. Without the split, an interrupt taken partway through main's
//! multiply re-enters the same scratch bytes and main resumes against the
//! ISR's state, with no diagnostic. See `split_isr_routines`.

use std::collections::{HashMap, HashSet};

use ir::{Alloca, BinOp, Block, Call, CallArg, FBinOp, FloatConvOp, Func, Icmp, Inst, Module, Param, Ty, Val};

pub fn legalize(m: Module) -> Module {
    // Interrupt duplication happens in two layers. User functions split
    // here, before the lowering loop, because their calls already exist.
    // The runtime routines split after it (`split_isr_routines`), because
    // the loop is what creates their calls.
    let m = duplicate_isr_shared(m);
    let mut funcs = Vec::with_capacity(m.funcs.len() + 16);
    let mut used: Vec<String> = Vec::new();
    // Fresh SSA names for the fcmp materialization intermediates (the call
    // dst and the icmp temps), seeded with every name the module defines so
    // the trees can never collide with a user reg.
    let mut names = FreshNames::from_module(&m);
    for f in m.funcs {
        let mut blocks = Vec::with_capacity(f.blocks.len());
        for b in f.blocks {
            let mut insts = Vec::with_capacity(b.insts.len());
            for inst in b.insts {
                match inst {
                    Inst::Bin(bin) => match fold_const_bin(&bin).or_else(|| lower_bin(&bin, &mut used)) {
                        Some(folded_or_call) => insts.push(folded_or_call),
                        None => insts.push(Inst::Bin(bin)),
                    },
                    Inst::Icmp(icmp) => match fold_const_icmp(&icmp) {
                        Some(frozen) => insts.push(frozen),
                        None => insts.push(Inst::Icmp(icmp)),
                    },
                    Inst::FloatBin(fb) => insts.push(lower_fbin(&fb, &mut used)),
                    Inst::Fcmp(fc) => insts.extend(lower_fcmp(&fc, &mut used, &mut names)),
                    Inst::FloatConv(fc) => insts.push(lower_fconv(&fc, &mut used)),
                    other => insts.push(other),
                }
            }
            blocks.push(Block { label: b.label, insts });
        }
        funcs.push(Func { name: f.name, ret: f.ret, params: f.params, blocks, isr: f.isr, naked: f.naked });
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
            .strip_suffix("_isr")
            .expect("legalize: isr routine name must end in _isr");
        let mut f = routine_func(base);
        f.name = name.clone();
        funcs.push(f);
    }
    Module { globals: m.globals, funcs, module_asm: m.module_asm }
}

/// Split the runtime routines that BOTH the main and interrupt contexts
/// reach: each gets an `_isr` copy and every routine call inside the ISR
/// context is rewritten to it. Returns the `_isr` names to inject, in
/// `used` order so the emitted module text stays deterministic.
///
/// A routine only the ISR reaches is left shared: there is no main-context
/// caller whose frame it could clobber, and a second copy would spend flash
/// and RAM for nothing. This mirrors `duplicate_isr_shared`'s policy for
/// user functions, one layer down.
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
    let isr_roots: Vec<&str> =
        funcs.iter().filter(|f| f.isr).map(|f| f.name.as_str()).collect();
    let isr_ctx = reachable(&isr_roots, &adj);
    let main_ctx = reachable(&["main"], &adj);
    let shared: HashSet<&str> = used
        .iter()
        .map(String::as_str)
        .filter(|r| isr_ctx.contains(*r) && main_ctx.contains(*r))
        .collect();
    if shared.is_empty() {
        return Vec::new();
    }
    // Rewrite the shared routines' calls inside the ISR context only. The
    // main-context callers (including a shared user function's ORIGINAL,
    // which duplicate_isr_shared left main-only) keep the base name.
    for f in funcs.iter_mut() {
        if !isr_ctx.contains(&f.name) {
            continue;
        }
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Call(c) = inst {
                    if shared.contains(c.func.as_str()) {
                        c.func = format!("{}_isr", c.func);
                    }
                }
            }
        }
    }
    used.iter()
        .filter(|u| shared.contains(u.as_str()))
        .map(|u| format!("{u}_isr"))
        .collect()
}

/// Fresh SSA name supply for the fcmp materialization trees. Seeded with
/// every name the module defines (params + inst dsts, module-wide — names
/// only need uniqueness inside a function, so the conservative seed merely
/// skips a few candidates), then hands out `c0`, `c1`, … skipping anything
/// already taken. Deterministic, so the lowered text round-trips.
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
        Inst::Icmp(i) => Some(&i.dst),
        Inst::Select(s) => Some(&s.dst),
        Inst::Call(c) => c.dst.as_deref(),
        Inst::Phi(p) => Some(&p.dst),
        Inst::Gep(g) => Some(&g.dst),
        Inst::Alloca(a) => Some(&a.dst),
        Inst::Freeze(f) => Some(&f.dst),
        Inst::FloatBin(b) => Some(&b.dst),
        Inst::Fcmp(c) => Some(&c.dst),
        Inst::FloatConv(c) => Some(&c.dst),
        Inst::Asm(_) => None,
        Inst::Ret(_) | Inst::Store(_) | Inst::Br(_) | Inst::BrCond(_) | Inst::Memcpy(_) => None,
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
            CallArg { ty: Some(Ty::F32), val: b.a.clone(), byval: None, sret: false },
            CallArg { ty: Some(Ty::F32), val: b.b.clone(), byval: None, sret: false },
        ],
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
            CallArg { ty: Some(Ty::F32), val: c.a.clone(), byval: None, sret: false },
            CallArg { ty: Some(Ty::F32), val: c.b.clone(), byval: None, sret: false },
        ],
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
    })
}

/// The per-predicate materialization tree over the `__cmp_f32` tri-state
/// byte (0 = equal, 1 = a < b, 2 = a > b, 3 = unordered). Every tree is
/// either a single `icmp eq/ne i8 %c, <k>` or the OR of two equality
/// icmps materialized as `select i1 <c==k1>, i1 true, i1 <c==k2>` — no i1
/// binops (isel rejects them). The trees are documented in the legalize
/// tests and are the Task-3 isel contract.
///
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
/// `fcmp true`/`fcmp false` are compile-time constants (clang never emits
/// them) and panic loudly instead of materializing a call the isel cannot
/// remove.
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
        "true" | "false" => panic!("legalize: fcmp {pred} is a compile-time constant (clang never emits it)"),
        other => panic!("legalize: unknown fcmp predicate {other:?}"),
    }
    out
}

/// Rewrite one `Inst::FloatConv`. The int<->float conversions become calls
/// to the four conversion routines — the source/target width rides on the
/// call's types (the routine's slot is always 4 bytes; an i8/i16 source or
/// result uses the low bytes). `fpext`/`fptrunc` are f32→f32 (double ==
/// float on msp430) and become a plain `freeze` copy, no call. Anything
/// touching a non-f32 type is an f64 attempt and panics loudly.
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
        }),
        (FloatConvOp::FpToUi, Ty::F32, to @ (Ty::I8 | Ty::I16 | Ty::I32)) => Inst::Call(Call {
            dst,
            ty: Some(to),
            func: mark("__fptoui_f32", used),
            args: vec![CallArg { ty: Some(Ty::F32), val: c.val.clone(), byval: None, sret: false }],
        }),
        (FloatConvOp::SiToFp, from @ (Ty::I8 | Ty::I16 | Ty::I32), Ty::F32) => Inst::Call(Call {
            dst,
            ty: Some(Ty::F32),
            func: mark("__sitofp_f32", used),
            args: vec![CallArg { ty: Some(from), val: c.val.clone(), byval: None, sret: false }],
        }),
        (FloatConvOp::UiToFp, from @ (Ty::I8 | Ty::I16 | Ty::I32), Ty::F32) => Inst::Call(Call {
            dst,
            ty: Some(Ty::F32),
            func: mark("__uitofp_f32", used),
            args: vec![CallArg { ty: Some(from), val: c.val.clone(), byval: None, sret: false }],
        }),
        (FloatConvOp::Fpext | FloatConvOp::Fptrunc, Ty::F32, Ty::F32) => {
            Inst::Freeze(ir::Freeze { dst: c.dst.clone(), ty: Ty::F32, val: c.val.clone() })
        }
        other => panic!(
            "legalize: unsupported float conversion {other:?} (msp430 has no f64; fpext/fptrunc are f32->f32 only)"
        ),
    }
}

/// Every function transitively reachable from `roots` over the caller ->
/// callee map `adj` (the roots included). A visited set keeps a call cycle
/// (rejected loudly later by callgraph/alloc) from looping forever.
fn reachable(roots: &[&str], adj: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<&str> = roots.to_vec();
    while let Some(f) = stack.pop() {
        if !seen.insert(f.to_string()) {
            continue;
        }
        if let Some(cs) = adj.get(f) {
            for c in cs {
                if !seen.contains(c) {
                    stack.push(c);
                }
            }
        }
    }
    seen
}

/// The interrupt shared-function duplication (the M13 ruling): every function
/// reachable from BOTH the ISR context (the ISR + its transitive callees) and
/// the main context (main + its transitive callees) gets an `_isr` copy — a
/// DEEP clone of the Func, renamed `{name}_isr`, with the `isr` flag cleared
/// (the copy is an ordinary function, not a second vector entry) — and every
/// call inside the ISR context whose target is a duplicated function is
/// rewritten to the `_isr` name (a copy's own calls to another shared
/// function become its `_isr` copy too; a non-shared ISR-context callee's
/// calls are rewritten as well — the whole ISR context runs against the
/// copies). The original shared functions stay main-context-only with their
/// calls untouched. Gated on the ISR's existence: a module with no ISR
/// passes through byte-identical, and so does a module whose ISR shares
/// nothing with main.
///
/// The call graph is re-derived locally from the module's CALL insts rather
/// than depending on the callgraph crate — it is a tiny, stable scan and
/// legalize already owns the module (no new dependency).
fn duplicate_isr_shared(m: Module) -> Module {
    let isr_names: HashSet<&str> = m.funcs.iter().filter(|f| f.isr).map(|f| f.name.as_str()).collect();
    if isr_names.is_empty() {
        return m;
    }

    // Caller -> callee edges from the CALL insts.
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for f in &m.funcs {
        adj.entry(f.name.clone()).or_default();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    adj.entry(f.name.clone()).or_default().push(c.func.clone());
                }
            }
        }
    }

    // The ISR context = the ISR + its transitive callees; the main context =
    // main + its transitive callees. Every function in BOTH (never the ISR
    // itself, never main) is duplicated.
    let isr_ctx = isr_names.iter().flat_map(|r| reachable(&[r], &adj)).collect::<HashSet<String>>();
    let main_ctx = reachable(&["main"], &adj);
    // main is excluded from the duplication above, so an ISR that
    // (transitively) calls main would leave the ISR's call on the original
    // `main` — re-entering the main context and silently collapsing the
    // disjoint-region guarantee. Panic loudly rather than miscompile.
    assert!(
        !isr_ctx.contains("main"),
        "isel/legalize: the ISR context must not reach main — re-entrant main is unsupported"
    );
    let shared: Vec<String> = m
        .funcs
        .iter()
        .filter(|f| !f.isr && f.name != "main")
        .filter(|f| isr_ctx.contains(&f.name) && main_ctx.contains(&f.name))
        .map(|f| f.name.clone())
        .collect();
    if shared.is_empty() {
        return m;
    }

    // Deep-clone each shared func as `{name}_isr` (renamed, isr flag
    // cleared). A name collision with an existing function panics loudly.
    let mut funcs = m.funcs;
    let mut copies: Vec<Func> = Vec::with_capacity(shared.len());
    for name in &shared {
        let copy_name = format!("{name}_isr");
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
        copies.push(c);
    }

    // Rewrite every call inside the ISR context whose target is a duplicated
    // function to the `_isr` copy. The rewrite set is the ISR context minus
    // the original shared functions (now main-context-only — their calls stay
    // on the originals) plus the copies (their internal calls become the
    // `_isr` names transitively). The ISR root itself and non-shared ISR
    // callees are in the set, so the whole ISR context runs against copies.
    let shared_set: HashSet<&str> = shared.iter().map(String::as_str).collect();
    let mut rewrite_set: HashSet<String> = isr_ctx
        .iter()
        .filter(|n| n.as_str() != "main" && !shared_set.contains(n.as_str()))
        .cloned()
        .collect();
    for c in &copies {
        rewrite_set.insert(c.name.clone());
    }
    // The copies go into the module before the rewrite so their internal
    // calls are rewritten too (a copy's call to another shared function ->
    // its `_isr` copy, transitively).
    funcs.extend(copies);
    for f in &mut funcs {
        if !rewrite_set.contains(&f.name) {
            continue;
        }
        for b in &mut f.blocks {
            for inst in &mut b.insts {
                if let Inst::Call(c) = inst {
                    let target = c.func.clone();
                    if shared_set.contains(target.as_str()) {
                        c.func = format!("{target}_isr");
                    }
                }
            }
        }
    }
    Module { globals: m.globals, funcs, module_asm: m.module_asm }
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
/// Fold a `Bin` whose operands are both `Val::Const` into an `Inst::Freeze`
/// carrying the literal result. isel has no path for a const-const shape
/// (clang folds these upstream; only hand-written IR or a compiler-generated
/// corner reaches here with both sides constant) — several ops panic
/// outright and `sub` silently miscompiles by reading the second constant as
/// a file address. `Freeze` already copies a `Val::Const` into `dst`'s slot
/// via a plain `MOVLW`, so this needs no isel changes.
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
    // that guard — leave it unfolded so isel's existing check still fires.
    if b.ty == Ty::I1 { return None }
    let (Val::Const(a), Val::Const(k)) = (&b.a, &b.b) else { return None };
    let width = u32::from(b.ty.bytes()) * 8;
    let result = eval_binop(b.op, width, *a, *k)?;
    Some(Inst::Freeze(ir::Freeze { dst: b.dst.clone(), ty: b.ty, val: Val::Const(result) }))
}

/// Same fold for `Icmp`; the result is always `i1`.
fn fold_const_icmp(c: &Icmp) -> Option<Inst> {
    let (Val::Const(a), Val::Const(k)) = (&c.a, &c.b) else { return None };
    let width = u32::from(c.ty.bytes()) * 8;
    let result = eval_icmp(&c.pred, width, *a, *k);
    Some(Inst::Freeze(ir::Freeze { dst: c.dst.clone(), ty: Ty::I1, val: Val::Const(i64::from(result)) }))
}

fn const_mask(width: u32) -> u64 {
    if width >= 64 { u64::MAX } else { (1u64 << width) - 1 }
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

/// Evaluate a binop on two constants, masked/interpreted at `width` bits to
/// match isel's own per-byte truncation convention (`(k >> idx*8) & 0xFF`).
/// The result is re-masked to `width` bits too — the IR text has no type
/// tag on a bare constant, so a folded `add i8 200, 100` must read as `44`,
/// not the unmasked `300`, to be the "obvious" result the width implies.
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
            if bu == 0 { return None };
            canon_unsigned(au / bu, width)
        }
        BinOp::URem => {
            if bu == 0 { return None };
            canon_unsigned(au % bu, width)
        }
        BinOp::SDiv => {
            if bu == 0 { return None };
            let q = sign_extend(au, width).wrapping_div(sign_extend(bu, width));
            canon_signed(q as u64, width)
        }
        BinOp::SRem => {
            if bu == 0 { return None };
            let r = sign_extend(au, width).wrapping_rem(sign_extend(bu, width));
            canon_signed(r as u64, width)
        }
        BinOp::Shl | BinOp::LShr | BinOp::AShr => {
            if !(0..i64::from(width)).contains(&b) { return None };
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
            CallArg { ty: Some(b.ty), val: b.a.clone(), byval: None, sret: false },
            CallArg { ty: Some(b.ty), val: b.b.clone(), byval: None, sret: false },
        ],
    }))
}

fn param(name: &str, width: u8) -> Param {
    Param { name: name.into(), width, byval: None, sret: false, ptr: false }
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
/// slots, and use `__scr` strictly by offset. Every routine's frame must
/// stay inside ONE GPR bank (any bank, issue #6), because the recipes'
/// loops are skip-sensitive: no BANKSEL may be inserted between a test and
/// its target or inside a carry idiom. `alloc` rounds a routine's base into
/// a single bank; `isel` verifies the placement.
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
/// | `__add_f32`, `__sub_f32` | 14 | `sa`@0 (sign of a), `ea`@1 (biased exponent of a), `ma`@2-4 (24-bit mantissa of a with the implicit bit), `sb`@5, `eb`@6, `mb`@7-9 (same for b), `stick`@10 (sticky collector for the right-alignment shift), `cnt`@11 (alignment/normalize shift counter), `ta1`@12 / `ta2`@13 (the 24-bit fraction window; `ta0` reuses the dead `eb` slot at offset 6) |
/// | `__mul_f32` | 14 | `sign`@0 (result sign = sa XOR sb), `e`@1-2 (biased result exponent: e1+e2-127, 16-bit intermediate), `bk`@3-5 (multiplier backup — shifted to test bits), `cnt`@6 (loop counter, 24), `m`@7-10 (running product — the top 25 bits of the 24x24 product accumulate here), `spare`@11-13 (rounding scratch) |
/// | `__div_f32` | 12 | `sign`@0 (result sign = sa XOR sb), `e`@1-2 (biased result exponent: e1-e2+127, 16-bit intermediate), `rem`@3-6 (partial remainder — 4 bytes: the 24-bit rem shift can carry a bit), `den`@7-9 (denominator copy — the restoring subtract/restore reads this, the param slot stays untouched), `cnt`@10 (loop counter, 24), `spare`@11 (rounding scratch) |
/// | `__cmp_f32` | 6 | `tmp`@0-1 (byte-compare scratch), `flags`@2 (sign-state / NaN-check flags), `spare`@3-5 |
/// | `__uitofp_f32`, `__sitofp_f32` | 8 | `cnt`@0 (leading-1 shift counter), `e`@1-2 (biased result exponent: 127+31-shifts), `guard`@3 (the round/guard bit), `stick`@4 (sticky), `spare`@5-7 |
/// | `__fptoui_f32`, `__fptosi_f32` | 8 | `e`@0 (biased exponent), `cnt`@1 (right-shift count: 127-e+23), `m`@2-4 (mantissa working copy — shifted right in place), `sign`@5 (fptosi only), `spare`@6-7 |
///
/// Notes: div-by-zero is LLVM poison — the loop runs (den = 0 ⇒ quotient
/// 0xFFFF, remainder 0), any value is legal, no guard. Variable-shift counts
/// arrive unmasked and are masked to `width - 1` inside the routine. The
/// signed wrappers abs in place in the param slots (unsigned abs, so INT_MIN
/// is safe), run the unsigned divmod, then negate per the flags byte. The
/// soft-float routines take their operands in 4-byte slots (`a`/`b` are the
/// f32 bytes; `val` is the 4-byte int slot — an i8/i16 source or result uses
/// the low bytes), write the result to the retval slots, and use `__scr`
/// strictly by offset (Task-3 recipe contract).
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
        // Milestone 15: the soft-float routines (f32 slots are 4 bytes).
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
            insts: vec![Inst::Alloca(Alloca { dst: "__scr".into(), size: scr })],
        }],
        isr: false, // runtime routines are never interrupt handlers
        naked: false,
    }
}
