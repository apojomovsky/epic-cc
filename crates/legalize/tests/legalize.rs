use ir::{parse, Inst};
use legalize::legalize;

#[test]
fn passes_8_bit_through() {
    let m = parse(
        "global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    ret void\n",
    );
    assert_eq!(legalize(m).funcs.len(), 1);
}

/// A module with `mul i16` + a variable `shl i16` + a const `shl i16`:
/// the mul lowers to a `call i16 @__mul_u16` (dst/ty preserved), the
/// variable-count shift lowers to a `call i16 @__shl_u16`, the const-count
/// shift stays a `shl i16` Bin (isel inlines it), and the two used routine
/// defs are injected (params + scratch alloca) while unused routines are not.
#[test]
fn lowers_mul_and_variable_shifts_to_runtime_calls() {
    let m = parse(
        "global in i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @in\n\
             %m = mul i16 %a, 7\n\
             %v = shl i16 %a, %a\n\
             %k = shl i16 %a, 3\n\
             store i16 %m, @in\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // The mul became a call with the same dst/ty and both args.
    assert!(text.contains("%m = call i16 @__mul_u16(i16 %a, i16 7)"));
    // The variable-count shift became a call to the shift routine.
    assert!(text.contains("%v = call i16 @__shl_u16(i16 %a, i16 %a)"));
    // The const-count shift stayed a Bin (isel inlines it).
    assert!(text.contains("%k = shl i16 %a 3"));
    // The used routine defs are injected with their scratch allocas.
    assert!(
        text.contains("fn __mul_u16(i16) (a=i16, b=i16)\n  block entry:\n    %__scr = alloca 14")
    );
    assert!(text
        .contains("fn __shl_u16(i16) (val=i16, cnt=i16)\n  block entry:\n    %__scr = alloca 4"));
    // Unused routines are NOT injected.
    assert!(!text.contains("__udiv_u8"));
}

/// The injected routine Funcs carry the exact scratch alloca sizes, and
/// `alloc::allocate` places each routine's params first and then the
/// `__scr` buffer right after them in the frame (proving the injection
/// sizes the routine frame correctly).
#[test]
fn injected_routines_get_param_and_scratch_slots_allocated() {
    let m = parse(
        "global in i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @in\n\
             %m = mul i16 %a, 7\n\
             %v = shl i16 %a, %a\n\
             %k = shl i16 %a, 3\n\
             store i16 %m, @in\n\
             ret void\n",
    );
    let m2 = legalize(m);
    // Exact injected sizes (the cross-task contract for Tasks 3/4).
    let mul = m2.funcs.iter().find(|f| f.name == "__mul_u16").unwrap();
    assert_eq!(mul.ret, Some(ir::Ty::I16));
    assert_eq!(mul.params.len(), 2);
    assert_eq!(mul.blocks[0].insts.len(), 1);
    match &mul.blocks[0].insts[0] {
        Inst::Alloca(a) => {
            assert_eq!(a.dst, "__scr");
            assert_eq!(a.size, 14);
        }
        other => panic!("__mul_u16 must inject the scratch alloca, got {other:?}"),
    }
    let shl = m2.funcs.iter().find(|f| f.name == "__shl_u16").unwrap();
    assert_eq!(shl.blocks[0].insts.len(), 1);
    match &shl.blocks[0].insts[0] {
        Inst::Alloca(a) => {
            assert_eq!(a.dst, "__scr");
            assert_eq!(a.size, 4);
        }
        other => panic!("__shl_u16 must inject the scratch alloca, got {other:?}"),
    }
    // Overlay placement: params first (2+2 bytes), then the __scr buffer.
    let out = alloc::allocate(
        &device::PIC16F877A,
        &m2,
        "edge main __mul_u16\nedge main __shl_u16\n",
    );
    assert_eq!(out.locals["__mul_u16::b"], out.locals["__mul_u16::a"] + 2);
    assert_eq!(
        out.locals["__mul_u16::__scr"],
        out.locals["__mul_u16::b"] + 2
    );
    assert_eq!(
        out.locals["__shl_u16::cnt"],
        out.locals["__shl_u16::val"] + 2
    );
    assert_eq!(
        out.locals["__shl_u16::__scr"],
        out.locals["__shl_u16::cnt"] + 2
    );
}

/// Table-driven: every mul/div/rem binop and every reg-count shift on i8/i16
/// lowers to a call to the matching runtime routine, and the injected Func
/// (signature: ret + 2 params) carries the exact scratch alloca size from the
/// Task-2 layout contract. Also asserts a const-count shift stays a `Bin`
/// (isel inlines it, so legalize must not rewrite it).
#[test]
fn pins_all_runtime_routine_mappings() {
    use ir::Ty;
    // (op, ty text, ty, routine, param names, __scr size)
    let cases: &[(&str, &str, Ty, &str, &[&str], u8)] = &[
        ("mul", "i8", Ty::I8, "__mul_u8", &["a", "b"], 6),
        ("mul", "i16", Ty::I16, "__mul_u16", &["a", "b"], 14),
        ("udiv", "i8", Ty::I8, "__udiv_u8", &["num", "den"], 4),
        ("udiv", "i16", Ty::I16, "__udiv_u16", &["num", "den"], 7),
        ("urem", "i8", Ty::I8, "__urem_u8", &["num", "den"], 4),
        ("urem", "i16", Ty::I16, "__urem_u16", &["num", "den"], 7),
        ("sdiv", "i8", Ty::I8, "__sdiv_i8", &["num", "den"], 5),
        ("sdiv", "i16", Ty::I16, "__sdiv_i16", &["num", "den"], 7),
        ("srem", "i8", Ty::I8, "__srem_i8", &["num", "den"], 5),
        ("srem", "i16", Ty::I16, "__srem_i16", &["num", "den"], 7),
        ("shl", "i8", Ty::I8, "__shl_u8", &["val", "cnt"], 3),
        ("shl", "i16", Ty::I16, "__shl_u16", &["val", "cnt"], 4),
        ("lshr", "i8", Ty::I8, "__lshr_u8", &["val", "cnt"], 3),
        ("lshr", "i16", Ty::I16, "__lshr_u16", &["val", "cnt"], 4),
        ("ashr", "i8", Ty::I8, "__ashr_i8", &["val", "cnt"], 3),
        ("ashr", "i16", Ty::I16, "__ashr_i16", &["val", "cnt"], 4),
        ("mul", "i32", Ty::I32, "__mul_u32", &["a", "b"], 11),
        ("udiv", "i32", Ty::I32, "__udiv_u32", &["num", "den"], 10),
        ("urem", "i32", Ty::I32, "__urem_u32", &["num", "den"], 10),
        ("sdiv", "i32", Ty::I32, "__sdiv_i32", &["num", "den"], 12),
        ("srem", "i32", Ty::I32, "__srem_i32", &["num", "den"], 12),
        ("shl", "i32", Ty::I32, "__shl_u32", &["val", "cnt"], 2),
        ("lshr", "i32", Ty::I32, "__lshr_u32", &["val", "cnt"], 2),
        ("ashr", "i32", Ty::I32, "__ashr_i32", &["val", "cnt"], 2),
    ];
    for (op, ty, ty_enum, routine, params, size) in cases {
        let src = format!(
            "global in {ty}\nfn main(void) ()\n  block entry:\n    %a = load {ty} @in\n    %b = load {ty} @in\n    %r = {op} {ty} %a, %b\n    ret void\n"
        );
        let m = legalize(parse(&src));
        let text = ir::serialize(&m);
        // (a) The Bin was rewritten to a Call of the correct routine, dst/ty
        // preserved and both operands passed as typed args.
        assert!(
            text.contains(&format!("%r = call {ty} @{routine}({ty} %a, {ty} %b)")),
            "{op} {ty}: expected call to {routine}, got:\n{text}"
        );
        // (b) The injected Func has the right signature: ret + 2 params with
        // the routine's parameter names and byte widths.
        let f = m
            .funcs
            .iter()
            .find(|f| f.name == *routine)
            .unwrap_or_else(|| panic!("{routine} not injected for {op} {ty}"));
        assert_eq!(f.ret, Some(*ty_enum), "{routine} return type");
        assert_eq!(f.params.len(), 2, "{routine} param count");
        for (i, pname) in params.iter().enumerate() {
            assert_eq!(f.params[i].name, *pname, "{routine} param {i} name");
            assert_eq!(
                f.params[i].width,
                ty_enum.bytes(),
                "{routine} param {i} width"
            );
        }
        // (c) The injected scratch alloca matches the Task-2 layout contract.
        assert_eq!(f.blocks.len(), 1, "{routine} block count");
        match &f.blocks[0].insts[0] {
            Inst::Alloca(a) => {
                assert_eq!(a.dst, "__scr", "{routine} scratch dst");
                assert_eq!(a.size, *size, "{routine} scratch size");
            }
            other => panic!("{routine}: expected scratch alloca, got {other:?}"),
        }
    }
    // A const-count shift stays a Bin (isel inlines the fixed sequence) — it
    // must not be rewritten to a call, and no routine is injected.
    let const_shift = legalize(parse(
        "global in i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @in\n    %k = shl i8 %a, 3\n    ret void\n",
    ));
    let ct = ir::serialize(&const_shift);
    assert!(ct.contains("%k = shl i8 %a 3"));
    assert!(!ct.contains("call"));
    assert!(!ct.contains("__shl_u8"));
}

/// The `func` targets of every CALL in `f`, in instruction order.
fn call_targets(f: &ir::Func) -> Vec<String> {
    let mut v = Vec::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::Call(c) = inst {
                v.push(c.func.clone());
            }
        }
    }
    v
}

fn func<'a>(name: &str, m: &'a ir::Module) -> &'a ir::Func {
    m.funcs
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("module has no function {name}"))
}

/// A module with main + isr both calling `helper`: the rewritten module has
/// `helper` (main's copy, untouched) AND `helper_isr` (a deep clone, renamed,
/// with the `isr` flag cleared); the ISR's call targets `helper_isr` while
/// main's call keeps targeting `helper`.
#[test]
fn duplicates_shared_functions_for_the_isr() {
    let m = parse(
        "global in i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             %h = load i8 @in\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    // Both the original and the _isr copy exist.
    assert!(names.contains(&"helper"), "helper must remain: {names:?}");
    assert!(
        names.contains(&"helper_isr"),
        "helper_isr must be added: {names:?}"
    );
    // The copy is a deep clone: same body, `isr` flag cleared.
    let helper_isr = func("helper_isr", &m2);
    assert!(!helper_isr.isr, "the _isr copy must not be marked isr");
    match &helper_isr.blocks[0].insts[0] {
        Inst::Load(l) => assert_eq!(l.dst, "h", "the copy must carry helper's body"),
        other => panic!("helper_isr must carry helper's body, got {other:?}"),
    }
    // The ISR's call targets the copy; main's call stays on the original.
    assert_eq!(call_targets(func("isr", &m2)), ["helper_isr"]);
    assert_eq!(call_targets(func("main", &m2)), ["helper"]);
    assert_eq!(call_targets(func("helper", &m2)), [] as [&str; 0]);
}

/// Transitivity: helper calls helper2 (both shared) — the copy's internal
/// call is rewritten to helper2_isr, while the original helper keeps calling
/// the original helper2. A non-shared ISR-context callee (isr_only) is NOT
/// duplicated, but its call to a shared function is rewritten to the copy
/// (the whole ISR context runs against the copies).
#[test]
fn rewrites_transitive_calls_and_skips_non_shared_callees() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             call void @helper2()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @helper()\n\
             call void @isr_only()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             call void @helper2()\n\
             ret void\n\
         fn helper2(void) ()\n\
           block entry:\n\
             ret void\n\
         fn isr_only(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    // Both shared functions got copies; the non-shared callee did not.
    assert!(
        names.contains(&"helper_isr"),
        "helper_isr missing: {names:?}"
    );
    assert!(
        names.contains(&"helper2_isr"),
        "helper2_isr missing: {names:?}"
    );
    assert!(
        names.contains(&"isr_only"),
        "isr_only must remain: {names:?}"
    );
    assert!(
        !names.contains(&"isr_only_isr"),
        "non-shared callee must NOT be duplicated: {names:?}"
    );
    // The copy's internal call to another shared function -> its _isr copy.
    assert_eq!(call_targets(func("helper_isr", &m2)), ["helper2_isr"]);
    // The original helper keeps calling the original helper2 (main's chain).
    assert_eq!(call_targets(func("helper", &m2)), ["helper2"]);
    // The non-shared ISR-context callee's call to a shared function is
    // rewritten to the copy.
    assert_eq!(call_targets(func("isr_only", &m2)), ["helper_isr"]);
    // Direct calls: main stays on the originals, the isr runs the copies.
    assert_eq!(call_targets(func("main", &m2)), ["helper", "helper2"]);
    assert_eq!(call_targets(func("isr", &m2)), ["helper_isr", "isr_only"]);
}

/// The rewritten module's canonical text round-trips (parse -> serialize is
/// a stable fixed point), and the copies show up in the text deliberately.
#[test]
fn duplicated_module_roundtrips_canonical_text() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("fn helper_isr(void) ()"),
        "missing copy:\n{text}"
    );
    assert!(
        text.contains("fn isr(void) [isr] ()"),
        "isr marker lost:\n{text}"
    );
    let m2 = parse(&text);
    assert_eq!(ir::serialize(&m2), text); // stable fixed point
}

/// The duplication is gated on the ISR's existence: without an ISR the
/// transform is a pass-through (byte-identical), and with an ISR whose
/// callees are all private there is nothing to duplicate.
#[test]
fn no_shared_function_means_no_duplication() {
    // No ISR at all: pass-through, byte-identical.
    let src = "fn main(void) ()\n  block entry:\n    call void @helper()\n    ret void\nfn helper(void) ()\n  block entry:\n    ret void\n";
    let m = parse(src);
    let text = ir::serialize(&legalize(m));
    assert!(
        !text.contains("_isr"),
        "no ISR: must not duplicate:\n{text}"
    );
    assert_eq!(
        text,
        ir::serialize(&parse(src)),
        "no ISR: must be byte-identical"
    );
    // An ISR with only private callees: nothing is shared, so nothing is
    // duplicated and the ISR's calls stay untouched.
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @isr_private()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             ret void\n\
         fn isr_private(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(
        !names.contains(&"helper_isr"),
        "helper is not shared: {names:?}"
    );
    assert!(
        !names.contains(&"isr_private_isr"),
        "isr_private is not shared: {names:?}"
    );
    assert_eq!(call_targets(func("isr", &m2)), ["isr_private"]);
    assert_eq!(call_targets(func("main", &m2)), ["helper"]);
}

/// main is excluded from the shared-function duplication, so an ISR that
/// (transitively) calls main would leave the ISR's call on the original
/// `main` — re-entering the main context and silently collapsing the
/// disjoint-region guarantee. duplicate_isr_shared must panic loudly
/// instead of miscompiling.
#[test]
#[should_panic(expected = "re-entrant main is unsupported")]
fn isr_context_reaching_main_panics_loudly() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @main()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let _ = legalize(m);
}

// ===== Milestone 15: the float lowering (the soft-float runtime calls) =====

/// The f1.ll float shapes: every f32 arithmetic op becomes a call to the
/// matching runtime routine with the dst/ty preserved and both operands
/// passed as float args; no `fadd`/`fdiv` Bin remains.
#[test]
fn lowers_float_arith_to_runtime_calls() {
    let m = parse(
        "global in float\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load float @in\n\
             %b = load float @in\n\
             %r1 = fadd float %a %b\n\
             %r2 = fsub float %a %b\n\
             %r3 = fmul float %a %b\n\
             %r4 = fdiv float %a %b\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // dst/ty preserved; both operands copied as float args.
    assert!(text.contains("%r1 = call float @__add_f32(float %a, float %b)"));
    assert!(text.contains("%r2 = call float @__sub_f32(float %a, float %b)"));
    assert!(text.contains("%r3 = call float @__mul_f32(float %a, float %b)"));
    assert!(text.contains("%r4 = call float @__div_f32(float %a, float %b)"));
    // The arithmetic insts are gone.
    assert!(!text.contains("fadd float"));
    assert!(!text.contains("fsub float"));
    assert!(!text.contains("fmul float"));
    assert!(!text.contains("fdiv float"));
}

/// `fcmp` becomes `%c = call i8 @__cmp_f32(a, b)` + the per-predicate
/// icmp/select tree over the tri-state byte (0=eq/1=lt/2=gt/3=unordered),
/// with the OR predicates materialized as `select i1 <c==k1>, i1 true,
/// i1 <c==k2>` — no i1 binops (the isel rejects them). Assert the exact
/// tree shapes for olt, oeq, one, ugt, ord, uno.
#[test]
fn lowers_fcmp_to_cmp_call_and_materialization_tree() {
    let m = parse(
        "global in float\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load float @in\n\
             %b = load float @in\n\
             %x1 = fcmp olt float %a %b\n\
             %x2 = fcmp oeq float %a %b\n\
             %x3 = fcmp one float %a %b\n\
             %x4 = fcmp ugt float %a %b\n\
             %x5 = fcmp ord float %a %b\n\
             %x6 = fcmp uno float %a %b\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // olt = (c==1): a single icmp on the call result.
    assert!(
        text.contains("%c0 = call i8 @__cmp_f32(float %a, float %b)\n    %x1 = icmp eq i8 %c0 1")
    );
    // oeq = (c==0).
    assert!(
        text.contains("%c1 = call i8 @__cmp_f32(float %a, float %b)\n    %x2 = icmp eq i8 %c1 0")
    );
    // one = (c==1) || (c==2): two icmps + a select (the OR materialization).
    assert!(text.contains("%c2 = call i8 @__cmp_f32(float %a, float %b)"));
    assert!(text.contains("%c3 = icmp eq i8 %c2 1"));
    assert!(text.contains("%c4 = icmp eq i8 %c2 2"));
    assert!(text.contains("%x3 = select i1 %c3 i1 1 i1 %c4"));
    // ugt = (c==2) || (c==3).
    assert!(text.contains("%c5 = call i8 @__cmp_f32(float %a, float %b)"));
    assert!(text.contains("%c6 = icmp eq i8 %c5 2"));
    assert!(text.contains("%c7 = icmp eq i8 %c5 3"));
    assert!(text.contains("%x4 = select i1 %c6 i1 1 i1 %c7"));
    // ord = (c!=3): a single icmp.
    assert!(
        text.contains("%c8 = call i8 @__cmp_f32(float %a, float %b)\n    %x5 = icmp ne i8 %c8 3")
    );
    // uno = (c==3).
    assert!(
        text.contains("%c9 = call i8 @__cmp_f32(float %a, float %b)\n    %x6 = icmp eq i8 %c9 3")
    );
    // No i1 binops anywhere — the ORs are selects, never `or i1`.
    assert!(!text.contains("or i1"), "i1 binops are forbidden:\n{text}");
    assert!(!text.contains("and i1"), "i1 binops are forbidden:\n{text}");
}

/// Table-driven: every one of the 14 fcmp predicates materializes as the
/// documented icmp/select tree over the `__cmp_f32` tri-state byte —
/// either one `icmp eq/ne i8 %c, <k>` or an OR `(c==k1)||(c==k2)` via two
/// icmps + a select. The trees are the Task-3 isel contract.
#[test]
fn pins_all_fcmp_predicate_trees() {
    // (pred, single icmp (op, k) | OR of two eqs (k1, k2))
    enum Tree {
        Icmp(&'static str, i64),
        Or(i64, i64),
    }
    use Tree::*;
    let cases: &[(&str, Tree)] = &[
        ("oeq", Icmp("eq", 0)),
        ("ogt", Icmp("eq", 2)),
        ("oge", Or(2, 0)),
        ("olt", Icmp("eq", 1)),
        ("ole", Or(1, 0)),
        ("one", Or(1, 2)),
        ("ord", Icmp("ne", 3)),
        ("ueq", Or(0, 3)),
        ("ugt", Or(2, 3)),
        ("uge", Icmp("ne", 1)),
        ("ult", Or(1, 3)),
        ("ule", Icmp("ne", 2)),
        ("une", Icmp("ne", 0)),
        ("uno", Icmp("eq", 3)),
    ];
    for (pred, tree) in cases {
        let src = format!(
            "global in float\nfn main(void) ()\n  block entry:\n    %a = load float @in\n    %b = load float @in\n    %r = fcmp {pred} float %a %b\n    ret void\n"
        );
        let text = ir::serialize(&legalize(parse(&src)));
        // The call comes first, into a fresh i8 dst.
        assert!(
            text.contains("%c0 = call i8 @__cmp_f32(float %a, float %b)"),
            "{pred}: missing __cmp_f32 call:\n{text}"
        );
        match tree {
            Icmp(op, k) => {
                assert!(
                    text.contains(&format!("%r = icmp {op} i8 %c0 {k}")),
                    "{pred}: expected single icmp {op} {k}:\n{text}"
                );
            }
            Or(k1, k2) => {
                assert!(
                    text.contains(&format!("%c1 = icmp eq i8 %c0 {k1}\n    %c2 = icmp eq i8 %c0 {k2}\n    %r = select i1 %c1 i1 1 i1 %c2")),
                    "{pred}: expected OR tree ((c=={k1})||(c=={k2})):\n{text}"
                );
            }
        }
        assert!(
            !text.contains("or i1"),
            "{pred}: i1 binop forbidden:\n{text}"
        );
    }
}

/// The nine float routine Funcs are injected with the EXACT signatures and
/// scratch alloca sizes from the Task-2 layout contract (the Task-3 isel
/// recipes read their working state from `{func}::__scr` + offset).
#[test]
fn injects_float_routines_with_exact_scratch_sizes() {
    use ir::Ty;
    // (name, ret, param names, param width, __scr size)
    let cases: &[(&str, Option<Ty>, &[&str], u8, u8)] = &[
        ("__add_f32", Some(Ty::F32), &["a", "b"], 4, 14),
        ("__sub_f32", Some(Ty::F32), &["a", "b"], 4, 14),
        ("__mul_f32", Some(Ty::F32), &["a", "b"], 4, 14),
        ("__div_f32", Some(Ty::F32), &["a", "b"], 4, 12),
        ("__cmp_f32", Some(Ty::I8), &["a", "b"], 4, 6),
        ("__uitofp_f32", Some(Ty::F32), &["val"], 4, 8),
        ("__sitofp_f32", Some(Ty::F32), &["val"], 4, 8),
        ("__fptoui_f32", Some(Ty::I32), &["val"], 4, 8),
        ("__fptosi_f32", Some(Ty::I32), &["val"], 4, 8),
    ];
    let m = parse(
        "global in float\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load float @in\n\
             %b = load float @in\n\
             %r1 = fadd float %a %b\n\
             %r2 = fsub float %a %b\n\
             %r3 = fmul float %a %b\n\
             %r4 = fdiv float %a %b\n\
             %r5 = fcmp olt float %a %b\n\
             %r6 = fptosi float %a to i16\n\
             %r7 = fptoui float %a to i32\n\
             %r8 = sitofp i16 %r6 to float\n\
             %r9 = uitofp i32 %r7 to float\n\
             ret void\n",
    );
    let m2 = legalize(m);
    for (name, ret, params, width, size) in cases {
        let f = m2
            .funcs
            .iter()
            .find(|f| f.name == *name)
            .unwrap_or_else(|| panic!("{name} not injected"));
        assert_eq!(&f.ret, ret, "{name} return type");
        assert_eq!(f.params.len(), params.len(), "{name} param count");
        for (i, pname) in params.iter().enumerate() {
            assert_eq!(f.params[i].name, *pname, "{name} param {i} name");
            assert_eq!(f.params[i].width, *width, "{name} param {i} width");
        }
        assert_eq!(f.blocks.len(), 1, "{name} block count");
        match &f.blocks[0].insts[0] {
            Inst::Alloca(a) => {
                assert_eq!(a.dst, "__scr", "{name} scratch dst");
                assert_eq!(a.size, *size, "{name} scratch size");
            }
            other => panic!("{name}: expected scratch alloca, got {other:?}"),
        }
    }
    // The injected defs carry the canonical text form too.
    let text = ir::serialize(&m2);
    assert!(
        text.contains("fn __add_f32(float) (a=i32, b=i32)\n  block entry:\n    %__scr = alloca 14")
    );
    assert!(text.contains("fn __cmp_f32(i8) (a=i32, b=i32)\n  block entry:\n    %__scr = alloca 6"));
    assert!(text.contains("fn __fptosi_f32(i32) (val=i32)\n  block entry:\n    %__scr = alloca 8"));
    assert!(
        text.contains("fn __uitofp_f32(float) (val=i32)\n  block entry:\n    %__scr = alloca 8")
    );
    // Only the used routines are injected (no integer routines here).
    assert!(!text.contains("__mul_u8"));
    assert!(!text.contains("__udiv_u16"));
}

/// The int<->float conversions become calls to the four conversion
/// routines (the source/target width rides on the call's ty — i8/i16/i32
/// sources use the low bytes of the 4-byte param slot); fpext/fptrunc
/// (f32->f32 — double == float on msp430) become plain freeze copies with
/// no call.
#[test]
fn lowers_float_conversions_and_casts() {
    let m = parse(
        "global in float\n\
         global ini i16\n\
         global ini32 i32\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load float @in\n\
             %v16 = load i16 @ini\n\
             %v32 = load i32 @ini32\n\
             %r1 = fptosi float %a to i16\n\
             %r2 = fptoui float %a to i32\n\
             %r3 = sitofp i16 %v16 to float\n\
             %r4 = uitofp i32 %v32 to float\n\
             %r5 = fpext float %r3 to float\n\
             %r6 = fptrunc float %r4 to float\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // fptosi/fptoui: the float operand is the routine's arg, the int width
    // is the call's return type.
    assert!(text.contains("%r1 = call i16 @__fptosi_f32(float %a)"));
    assert!(text.contains("%r2 = call i32 @__fptoui_f32(float %a)"));
    // sitofp/uitofp: the int operand is the routine's arg (typed, so the
    // isel copies only its width into the 4-byte val slot), the result float.
    assert!(text.contains("%r3 = call float @__sitofp_f32(i16 %v16)"));
    assert!(text.contains("%r4 = call float @__uitofp_f32(i32 %v32)"));
    // fpext/fptrunc are plain copies — freeze, never a call.
    assert!(text.contains("%r5 = freeze float %r3"));
    assert!(text.contains("%r6 = freeze float %r4"));
    assert!(!text.contains("fpext"));
    assert!(!text.contains("fptrunc"));
    assert!(!text.contains("call float @__fpext"));
}

/// The lowered module's canonical text round-trips (parse -> serialize is a
/// stable fixed point) — the injected routine defs and the fcmp trees show
/// up in the text deliberately.
#[test]
fn float_lowering_roundtrips_canonical_text() {
    let m = parse(
        "global in float\n\
         fn fadd(float) (a=float, b=float)\n\
           block entry:\n\
             %1 = fadd float %a %b\n\
             ret float %1\n\
         fn fcmp1(float) (a=float, b=float)\n\
           block entry:\n\
             %2 = fcmp oeq float %a %b\n\
             %3 = fcmp one float %a %b\n\
             %4 = fcmp ugt float %a %b\n\
             %5 = fcmp ord float %a %b\n\
             %6 = fcmp uno float %a %b\n\
             ret void\n\
         fn fconv(float) (a=float)\n\
           block entry:\n\
             %7 = fptosi float %a to i16\n\
             %8 = fptoui float %a to i32\n\
             %9 = sitofp i16 %7 to float\n\
             %10 = uitofp i32 %8 to float\n\
             %11 = fpext float %9 to float\n\
             %12 = fptrunc float %10 to float\n\
             ret float %11\n",
    );
    let text = ir::serialize(&legalize(m));
    let m2 = parse(&text);
    assert_eq!(ir::serialize(&m2), text, "stable fixed point\n---\n{text}");
    for line in [
        "fn __add_f32(float) (a=i32, b=i32)",
        "%1 = call float @__add_f32(float %a, float %b)",
        "%c0 = call i8 @__cmp_f32(float %a, float %b)",
        "%2 = icmp eq i8 %c0 0",
        "%7 = call i16 @__fptosi_f32(float %a)",
        "%9 = call float @__sitofp_f32(i16 %7)",
        "%11 = freeze float %9",
        "fn __cmp_f32(i8) (a=i32, b=i32)",
        "fn __fptosi_f32(i32) (val=i32)",
    ] {
        assert!(
            text.contains(line),
            "missing canonical line: {line}\n---\n{text}"
        );
    }
}

/// Issue #2: a runtime routine reachable from BOTH main and the ISR must be
/// duplicated the same way a shared user function is. Without the copy, an
/// ISR that preempts main inside `__mul_u8` re-enters the one shared frame
/// and clobbers main's in-flight state.
#[test]
fn duplicates_shared_runtime_routines_for_the_isr() {
    let m = parse(
        "global a i8\n\
         global b i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load i8 @a\n\
             %y = load i8 @b\n\
             %p = mul i8 %x, %y\n\
             store i8 %p @out\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %u = load i8 @a\n\
             %v = load i8 @b\n\
             %q = mul i8 %u, %v\n\
             store i8 %q @out\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"__mul_u8"),
        "main's routine must remain: {names:?}"
    );
    assert!(
        names.contains(&"__mul_u8_isr"),
        "the ISR needs its own routine copy: {names:?}"
    );
    // The ISR calls its copy; main keeps the original.
    assert_eq!(call_targets(func("isr", &m2)), ["__mul_u8_isr"]);
    assert_eq!(call_targets(func("main", &m2)), ["__mul_u8"]);
    // The copy carries the same ABI as the original (params + scratch), so
    // alloc sizes it an independent frame.
    let orig = func("__mul_u8", &m2);
    let copy = func("__mul_u8_isr", &m2);
    assert!(!copy.isr, "the routine copy must not be marked isr");
    assert_eq!(
        copy.ret, orig.ret,
        "the copy keeps the routine's return type"
    );
    assert_eq!(
        copy.params.len(),
        orig.params.len(),
        "the copy keeps the routine's params"
    );
    match (&copy.blocks[0].insts[0], &orig.blocks[0].insts[0]) {
        (Inst::Alloca(c), Inst::Alloca(o)) => {
            assert_eq!(c.dst, "__scr");
            assert_eq!(c.size, o.size, "the copy keeps the routine's scratch size");
        }
        other => panic!("the routine copy must hold a scratch alloca, got {other:?}"),
    }
}

/// A routine used ONLY by the ISR is not duplicated: there is no main-context
/// caller to clobber, so a second copy would waste flash and RAM.
#[test]
fn does_not_duplicate_isr_only_runtime_routines() {
    let m = parse(
        "global a i8\n\
         global b i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load i8 @a\n\
             store i8 %x @out\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %u = load i8 @a\n\
             %v = load i8 @b\n\
             %q = mul i8 %u, %v\n\
             store i8 %q @out\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"__mul_u8"),
        "the routine must be injected: {names:?}"
    );
    assert!(
        !names.contains(&"__mul_u8_isr"),
        "an ISR-only routine must NOT be duplicated: {names:?}"
    );
    assert_eq!(call_targets(func("isr", &m2)), ["__mul_u8"]);
}

// Issue #10: hand-written IR (or a compiler-generated shape clang didn't
// fold) can reach legalize with a Bin/Icmp whose operands are both
// Val::Const. isel has no path for this shape — several ops panic outright
// ("constant folding not implemented" / "needs a register operand"), and
// `sub` silently miscompiles by reading the second constant as a bogus file
// address. Folding at legalize means isel never sees the shape at all.

#[test]
fn leaves_i1_bin_unfolded_for_isels_existing_type_guard() {
    // i1 is icmp/fcmp's output type only. isel asserts `b.ty != Ty::I1` for
    // Bin because arithmetic bit-widths make no sense on a 1-bit value;
    // folding must not manufacture an out-of-range "i1" constant like 2 by
    // treating i1 as an 8-bit width the way Ty::bytes() does.
    let m = parse("fn main(void) ()\n  block entry:\n    %a = add i1 1, 1\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = add i1 1 1"),
        "must stay a Bin so isel's type guard still fires\n---\n{text}"
    );
}

#[test]
fn adds_two_constants_without_reaching_isel() {
    let m = parse("fn main(void) ()\n  block entry:\n    %a = add i8 200, 100\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    // 200 + 100 = 300, wraps to 44 in 8 bits.
    assert!(
        text.contains("%a = freeze i8 44"),
        "expected folded add\n---\n{text}"
    );
}

#[test]
fn subtracts_two_constants_without_reaching_isel() {
    // Previously a silent miscompile: emit_sub_const_lhs read the second
    // constant as a file-register address instead of computing k1 - k2.
    let m = parse("fn main(void) ()\n  block entry:\n    %a = sub i8 5, 3\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = freeze i8 2"),
        "expected folded sub\n---\n{text}"
    );
}

#[test]
fn folds_signed_division_with_truncation_toward_zero() {
    let m = parse("fn main(void) ()\n  block entry:\n    %a = sdiv i8 -7, 2\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = freeze i8 -3"),
        "expected -7/2 truncated to -3\n---\n{text}"
    );
}

#[test]
fn folds_mul_directly_instead_of_calling_the_runtime_routine() {
    let m = parse("fn main(void) ()\n  block entry:\n    %a = mul i8 6, 7\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = freeze i8 42"),
        "expected folded mul\n---\n{text}"
    );
    assert!(
        !text.contains("__mul_u8"),
        "must not call the runtime routine for a constant fold\n---\n{text}"
    );
}

#[test]
fn folds_icmp_predicates_on_two_constants() {
    let m = parse(
        "fn main(void) ()\n  block entry:\n\
         %a = icmp ult i8 3, 5\n\
         %b = icmp sgt i8 -1, 0\n\
         ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = freeze i1 1"),
        "3 ult 5 is true\n---\n{text}"
    );
    assert!(
        text.contains("%b = freeze i1 0"),
        "-1 sgt 0 (signed) is false\n---\n{text}"
    );
}

#[test]
fn leaves_division_by_a_zero_constant_as_the_existing_poison_call() {
    // Div-by-zero is LLVM poison; the runtime routine already defines the
    // documented poison behavior, so folding must not invent a new one.
    let m = parse("fn main(void) ()\n  block entry:\n    %a = udiv i8 5, 0\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = call i8 @__udiv_u8(i8 5, i8 0)"),
        "must fall back to the routine call, unfolded\n---\n{text}"
    );
}

#[test]
fn leaves_an_out_of_range_shift_count_unfolded_for_isels_existing_poison_check() {
    let m = parse("fn main(void) ()\n  block entry:\n    %a = shl i8 5, 10\n    ret void\n");
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%a = shl i8 5 10"),
        "must stay a Bin so isel's poison assert still fires\n---\n{text}"
    );
}

#[test]
fn sinks_ptr_select_into_caller_and_drops_func() {
    // ccp_sel's noinline shape: a pointer-returning function whose body is
    // `icmp` + pointer select + ret. Legalize sinks it into the caller: the
    // call disappears, the caller carries the select chain, and the callee
    // is dropped (no callers remain).
    let m = parse(
        "global addrs i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @addrs\n\
             %r = call i16 @ccp_sel(i8 %i)\n\
             store i8 0, %r\n\
             ret void\n\
         fn ccp_sel(i16) (0=i8)\n\
           block entry:\n\
             %2 = icmp eq i8 %0, 1\n\
             %g = gep @addrs +4\n\
             %3 = select i1 %2, ptr %g, ptr @addrs\n\
             ret i16 %3\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        !text.contains("@ccp_sel("),
        "the sunk function must be dropped:\n{text}"
    );
    assert!(
        !text.contains("call"),
        "the caller's call must be replaced:\n{text}"
    );
    assert!(
        text.contains("%r = select i1 %c0 ptr %c1 ptr @addrs"),
        "the select chain must be in the caller:\n{text}"
    );
}

#[test]
fn keeps_a_non_sinkable_ptr_func() {
    // A pointer-returning function with a body that is not a select of
    // constant arms (here a ret of a plain param) is not sinkable; it stays
    // in the module.
    let m = parse(
        "global addrs i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %r = call i16 @identity(@addrs)\n\
             ret void\n\
         fn identity(i16) (0=ptr)\n\
           block entry:\n\
             ret i16 %0\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@identity"),
        "a non-sinkable pointer func must stay:\n{text}"
    );
}
