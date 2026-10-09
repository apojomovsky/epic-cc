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
/// A callback stored by main into a global the ISR reads (the cross-context
/// shape, epic-cc#137): the ISR's indirect call site gets the stored
/// callback as a candidate, the shared callback is `_isr`-duplicated, and
/// main's store is rewritten to the copy so the ISR dispatches inside the
/// disjoint ISR region.
#[test]
fn fills_cross_context_stored_callback_candidates() {
    let m = parse(
        "global g_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @g_cb\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    // The shared callback got an `_isr` copy.
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing"
    );
    // The ISR's indirect call site lists the callback as a candidate.
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
    // main's store now points at the `_isr` copy.
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb_isr".to_string()),
        "main store must point at the _isr copy"
    );
}
/// Dead main-context twin drop (epic-cc#780): the const-propagated
/// registration shape, where the callee stores constant `@cb` into an
/// ISR-read global while main still passes `@cb` into the callee's now
/// ignored parameter. The store rewrite moves the only live reference to
/// `cb_isr`; the dead argument respells to the copy and the original is
/// dropped, so the backend never emits it.
#[test]
fn drops_dead_main_context_twin_after_const_propagated_register() {
    let m = parse(
        "global g_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @register(i16 @cb)\n\
             ret void\n\
         fn register(void) (cb=i16)\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @g_cb\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"cb_isr"), "copy missing: {names:?}");
    assert!(!names.contains(&"cb"), "dead original must go: {names:?}");
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let reg_call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("main call");
    assert_eq!(
        reg_call.args[0].val,
        ir::Val::Global("cb_isr".to_string()),
        "dead argument must respell to the copy"
    );
}
/// A callback passed to a main-context dispatcher that calls through its
/// parameter is live even when the same callback is stored into an
/// ISR-read global elsewhere: the argument must stay on the original and
/// the original must survive, or main-context execution would run the
/// ISR-context copy.
#[test]
fn keeps_callback_dispatched_through_a_main_context_parameter() {
    let m = parse(
        "global g_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @dispatch(i16 @cb)\n\
             ret void\n\
         fn dispatch(void) (fp=i16)\n\
           block entry:\n\
             call void @fp()\n\
             ret void\n\
         fn holder(void) ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @g_cb\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"cb_isr"), "copy missing: {names:?}");
    assert!(names.contains(&"cb"), "live original must stay: {names:?}");
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("main call");
    assert_eq!(
        call.args[0].val,
        ir::Val::Global("cb".to_string()),
        "dispatched argument must stay on the original"
    );
}

/// A callback stored by main into a global the ISR does NOT read stays
/// main-only: no `_isr` copy, no ISR candidate, main's store untouched.
#[test]
fn main_only_stored_callback_stays_main_only() {
    let m = parse(
        "global g_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        !m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb must not be duplicated"
    );
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb".to_string()),
        "main store stays on the original"
    );
}

/// A callback that flows into an ISR-read global through a whole-struct
/// memcpy (the HAL `Init(&h)` idiom, epic-cc#463): main builds a local
/// handle via a field store (`h.OverflowCallback = cb;`), then a memcpy
/// copies the whole struct into a global the ISR reads through, one field
/// at a time. The direct-store cross-context rewrite alone never sees the
/// callback (the store that actually writes it targets the local alloca,
/// not the global), so before the fix `cb` was never duplicated and ran
/// with a main-context frame when the real ISR called it, corrupting
/// whatever main frame it collided with (PIC18F4550 tick ISR hang under
/// MPLAB SIM: the ISR never reached RETFIE).
#[test]
fn fills_memcpy_struct_copy_callback_candidates() {
    let m = parse(
        "global g_storage i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %h = alloca 4\n\
             %f1 = gep %h +2\n\
             store i16 @cb %f1\n\
             memcpy @g_storage %h 4\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %p = gep @g_storage +2\n\
             %1 = load i16 %p\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    // The struct-copied callback got an `_isr` copy.
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing: the memcpy write-edge was not detected"
    );
    // The ISR's indirect call site lists the callback as a candidate.
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
    // main's field store into the local handle now points at the `_isr`
    // copy: this is the rewrite that actually mattered for epic-cc#463,
    // since the candidate list alone is not enough if the runtime value stored
    // into the struct is still the original's address, which trips the
    // indirect call's "no matching candidate" trap at runtime instead of
    // reaching the callback.
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb_isr".to_string()),
        "main's struct-field store must point at the _isr copy"
    );
}

/// A callback flowing into an ISR-read global through an UNINLINED `Init(&h)`
/// helper (epic-cc#918): the caller stores the callback into a local alloca
/// while the callee memcpys its param into the storage, so neither the
/// same-function alloca scan nor the handle-global scan sees the edge. Left
/// unresolved, the tick callback missed the ISR site's candidate list and
/// the first Timer2 tick trapped in the no-match loop. Resolving the feed
/// one hop through the call edge duplicates the callback and scopes the
/// site to the copy.
#[test]
fn fills_uninlined_init_memcpy_callback_candidates() {
    let m = parse(
        "global g_storage i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %h = alloca 4\n\
             %f1 = gep %h +2\n\
             store i16 @cb %f1\n\
             call void @init(i16 %h)\n\
             ret void\n\
         fn init(void) (p=i16)\n\
           block entry:\n\
             memcpy @g_storage %p 4\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %p = gep @g_storage +2\n\
             %1 = load i16 %p\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing: the cross-function memcpy edge was not detected"
    );
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb_isr".to_string()),
        "caller's alloca-field store must point at the _isr copy"
    );
}

/// A callback that flows into an ISR-read global through a handle GLOBAL
/// memcpy'd whole-object inside an Init helper (the HAL `Init(&h)` idiom
/// with the handle as a file-scope static, epic-cc#484): main stores the
/// callback into the handle's callback field, `init` memcpy's the handle
/// into the ISR-read storage global, and the ISR calls through the
/// storage's field. Here the memcpy source is the handle global itself
/// (clang promotes Init's param when one call site feeds it).
#[test]
fn fills_global_handle_memcpy_callback_candidates() {
    let m = parse(
        "global g_h i8\n\
         global g_storage i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g_h +2\n\
             store i16 @cb %p\n\
             call void @init()\n\
             ret void\n\
         fn init(void) ()\n\
           block entry:\n\
             memcpy @g_storage @g_h 4\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %q = gep @g_storage +2\n\
             %1 = load i16 %q\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing: the handle-global memcpy write-edge was not detected"
    );
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(
        call.callees,
        vec!["cb_isr".to_string()],
        "the ISR site must dispatch the copy"
    );
    // main's store into the handle global points at the `_isr` copy: at
    // runtime the storage field carries the copy's address, so the ISR
    // enters the disjoint ISR-region frame, not the preemptable original.
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb_isr".to_string()),
        "main's handle-field store must point at the _isr copy"
    );
}

/// The same handle-memcpy flow with the memcpy source still Init's param
/// (the idiom before clang promotes it, e.g. several call sites): the
/// handle global is recovered from the call sites, and the callback joins
/// the ISR context exactly as in the promoted form.
#[test]
fn fills_param_handle_memcpy_callback_candidates() {
    let m = parse(
        "global g_h i8\n\
         global g_storage i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g_h +2\n\
             store i16 @cb %p\n\
             call void @init(i16 @g_h)\n\
             ret void\n\
         fn init(i16) (0=i16)\n\
           block entry:\n\
             memcpy @g_storage %0 4\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %q = gep @g_storage +2\n\
             %1 = load i16 %q\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing: the param-pointed handle memcpy write-edge was not detected"
    );
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
}

/// Two priorities dispatching through separate storages: each site lists
/// exactly its own priority's copy. The pre-#582 context-scoped filter
/// listed both spellings at both sites, so the other priority's copy rode
/// along as a dead candidate; storage scoping pins each list to the
/// storages the site actually reads (ADR-038).
#[test]
fn per_priority_storages_list_only_their_own_copies() {
    let m = parse(
        "global g_lo i8\n\
         global g_hi i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g_lo +2\n\
             store i16 @cb %p\n\
             %q = gep @g_hi +2\n\
             store i16 @cb %q\n\
             ret void\n\
         fn isr_lo(void) [isr] [irq2] ()\n\
           block entry:\n\
             %p = gep @g_lo +2\n\
             %1 = load i16 %p\n\
             call void @1()\n\
             ret void\n\
         fn isr_hi(void) [isr] [irq1] ()\n\
           block entry:\n\
             %q = gep @g_hi +2\n\
             %2 = load i16 %q\n\
             call void @2()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let sites = |fname: &str| {
        m2.funcs
            .iter()
            .find(|f| f.name == fname)
            .unwrap()
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Call(c) if !c.callees.is_empty() => Some(c.callees.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sites("isr_lo"),
        vec![vec!["cb_isr".to_string()]],
        "the lo site must list only the lo copy"
    );
    assert_eq!(
        sites("isr_hi"),
        vec![vec!["cb_isr_high".to_string()]],
        "the hi site must list only the hi copy"
    );
}
/// A storage with two callback fields where only one holds a tracked
/// spelling (the USART Tx/Rx shape: the harness registers Tx, Rx stays
/// null): the TX site scopes to its field's spelling, and the RX site,
/// loading a field with no tracked spelling, degrades to the legacy
/// list instead of panicking (epic-cc#467).
#[test]
fn unregistered_field_falls_back_instead_of_panicking() {
    let m = parse(
        "global g_usart i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g_usart +2\n\
             store i16 @tx_cb %p\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %q = gep @g_usart +2\n\
             %1 = load i16 %q\n\
             call void @1()\n\
             %r = gep @g_usart +4\n\
             %2 = load i16 %r\n\
             call void @2(i16 7)\n\
             ret void\n\
         fn tx_cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let sites = |fname: &str| {
        m2.funcs
            .iter()
            .find(|f| f.name == fname)
            .unwrap()
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Call(c) if !c.callees.is_empty() => Some(c.callees.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // The TX site (0-arg call into the registered field) lists exactly
    // its tracked spelling; the RX site's unregistered field degrades
    // to the legacy list, which admits nothing with matching arity, so
    // only the TX site appears. No panic either way.
    assert_eq!(
        sites("isr"),
        vec![vec!["tx_cb_isr".to_string()]],
        "the TX site must list its tracked spelling"
    );
}

/// A callback that flows into an ISR-read global through a function
/// parameter (the `EPIC_GPIO_RegisterChangeCallback(on_rb_change)` shape):
/// the call site's argument is rewritten to the `_isr` copy and the ISR
/// site gets the callback as a candidate.
#[test]
fn rewrites_param_forwarded_callback_argument() {
    let m = parse(
        "global s_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @register(i16 @cb)\n\
             ret void\n\
         fn register(i16) (0=i16)\n\
           block entry:\n\
             store i16 %0 @s_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @s_cb\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing"
    );
    // The call site's argument passes the `_isr` copy.
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("main call");
    assert_eq!(
        call.args[0].val,
        ir::Val::Global("cb_isr".to_string()),
        "call argument must point at the _isr copy"
    );
    // The ISR site lists the callback as a candidate.
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let isr_call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(isr_call.callees, vec!["cb_isr".to_string()]);
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

/// An indirect call site's `callees` is filled from the whole-program
/// address-taken set (every function whose address appears as a value),
/// sorted deterministically. A direct call keeps an empty list.
#[test]
fn fills_indirect_callees_from_address_taken_set() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @sel\n\
             %2 = icmp eq i8 %1 0\n\
             %3 = select i1 %2 ptr @f0 ptr @f1\n\
             %4 = call i8 %3()\n\
             call void @f2()\n\
             ret void\n\
         fn f0(void) ()\n  block entry:\n    ret void\n\
         fn f1(void) ()\n  block entry:\n    ret void\n\
         fn f2(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let calls: Vec<&ir::Call> = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| match i {
            ir::Inst::Call(c) => Some(c),
            _ => None,
        })
        .collect();
    // The indirect call's candidates are the address-taken functions, sorted.
    assert_eq!(calls[0].callees, vec!["f0".to_string(), "f1".to_string()]);
    // The direct call keeps an empty candidate list.
    assert!(calls[1].callees.is_empty(), "direct call has no callees");
}

/// A function-pointer VALUE referencing a shared function inside the ISR
/// context is rewritten to the `_isr` copy, so the ISR's stored callback
/// points at the copy (which runs in the disjoint ISR region), not the
/// main-context original.
#[test]
fn rewrites_shared_function_pointer_values_in_isr_context() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    // The shared function got an `_isr` copy.
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing"
    );
    // The ISR's store now references the copy; main's stays on the original.
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let isr_store = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            ir::Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("isr store");
    assert_eq!(
        isr_store.val,
        ir::Val::Global("cb_isr".to_string()),
        "ISR store must point at the _isr copy"
    );
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let main_store = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            ir::Inst::Store(s) => Some(s),
            _ => None,
        })
        .expect("main store");
    assert_eq!(
        main_store.val,
        ir::Val::Global("cb".to_string()),
        "main store stays on the original"
    );
}

/// The pid clamp intrinsics (`llvm.smax`/`llvm.smin`) lower to an
/// icmp/select tree, and `llvm.abs` to icmp + sub + select. The canonical
/// text round-trips the lowered shape, so the assertions are text-level
/// (matching the repo's other legalize tests).
#[test]
fn lowers_smax_smin_abs_to_icmp_select() {
    let m = parse(
        "global a i16\n\
         global b i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @a\n\
             %b = load i16 @b\n\
             %x = call i16 @llvm.smax.i16(i16 %a, i16 %b)\n\
             %y = call i16 @llvm.smin.i16(i16 %x, i16 %b)\n\
             %z = call i16 @llvm.abs.i16(i16 %y, i1 0)\n\
             store i16 %z, @a\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // smax: icmp sgt + select of the two operands (canonical text has
    // no commas between operands).
    assert!(text.contains("%c0 = icmp sgt i16 %a %b"), "{text}");
    assert!(text.contains("%x = select i1 %c0 i16 %a i16 %b"), "{text}");
    // smin: icmp slt + select.
    assert!(text.contains("%c1 = icmp slt i16 %x %b"), "{text}");
    assert!(text.contains("%y = select i1 %c1 i16 %x i16 %b"), "{text}");
    // abs: icmp slt 0, sub 0-a, select neg when negative.
    assert!(text.contains("%c2 = icmp slt i16 %y 0"), "{text}");
    assert!(text.contains("%c3 = sub i16 0 %y"), "{text}");
    assert!(text.contains("%z = select i1 %c2 i16 %c3 i16 %y"), "{text}");

    // Width-parametric: same lowering works for i8 and i32.
    let m2 = parse(
        "global a i8
         global b i8
         global c i32
         global d i32
         fn main(void) ()
           block entry:
             %a = load i8 @a
             %b = load i8 @b
             %c = load i32 @c
             %d = load i32 @d
             %x = call i8 @llvm.smax.i8(i8 %a, i8 %b)
             %y = call i32 @llvm.smin.i32(i32 %c, i32 %d)
             store i8 %x, @a
             store i32 %y, @c
             ret void
",
    );
    let text2 = ir::serialize(&legalize(m2));
    assert!(text2.contains("icmp sgt i8"), "{text2}");
    assert!(text2.contains("icmp slt i32"), "{text2}");
}

/// `llvm.bitreverse.i6` (clang 2-bit deposit idiom, epic-cc#679)
/// widens into a byte slot: the input is masked to six bits, each lane
/// is isolated and shifted to its mirror position, and the lanes are
/// or-ed together. Assertions are text-level like the other intrinsic tests.
#[test]
fn lowers_bitreverse_i6_to_masked_lanes() {
    let m = parse(
        "global a i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i8 @a\n\
             %r = call i8 @llvm.bitreverse.i6(i8 %a)\n\
             store i8 %r, @a\n\
             ret void\n\
",
    );
    let text = ir::serialize(&legalize(m));
    // The input mask re-applies trunc-to-i6 semantics.
    assert!(text.contains("and i8 %a 63"), "{text}");
    // One lane per bit: isolate, shift to the mirror position.
    assert!(text.contains("shl i8"), "{text}");
    assert!(text.contains("lshr i8"), "{text}");
    // The dst is the or of all six lanes; no intrinsic call remains.
    assert!(text.contains("%r = or i8"), "{text}");
    assert!(!text.contains("bitreverse"), "{text}");
}

/// An unknown `llvm.*` intrinsic panics loudly so a new clang-emitted
/// intrinsic surfaces as a clear error instead of a silent hole.
#[test]
#[should_panic(expected = "legalize: unknown intrinsic")]
fn unknown_intrinsic_panics() {
    let m = parse(
        "global a i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @a\n\
             %x = call i16 @llvm.sadd.sat.i16(i16 %a, i16 %a)\n\
             ret void\n",
    );
    let _ = legalize(m);
}
/// A cross-context callback that calls a shared helper: the store-edge
/// extension must close over callees, so the helper is `_isr`-duplicated
/// and the callback's copy calls the helper's copy (ADR-013: the ISR must
/// never run a main-context frame).
#[test]
fn closes_cross_context_extension_over_callees() {
    let m = parse(
        "global g_cb i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store i16 @cb @g_cb\n\
             call void @helper()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @g_cb\n\
             call void @1()\n\
             ret void\n\
         fn cb(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn helper(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    // Both the callback and its callee are duplicated.
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing"
    );
    assert!(
        m2.funcs.iter().any(|f| f.name == "helper_isr"),
        "helper_isr missing"
    );
    // The callback's copy calls the helper's copy.
    let cb_isr = m2.funcs.iter().find(|f| f.name == "cb_isr").unwrap();
    let call = cb_isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("cb_isr call");
    assert_eq!(call.func, "helper_isr");
}

/// A callee storing two params into two ISR-read globals: every call-site
/// argument is rewritten to the `_isr` copy, not just the first match.
#[test]
fn rewrites_every_param_forwarded_argument() {
    let m = parse(
        "global s_cb0 i16\n\
         global s_cb1 i16\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @register(i16 @cb0, i16 @cb1)\n\
             ret void\n\
         fn register(void) (0=i16, 1=i16)\n\
           block entry:\n\
             store i16 %0 @s_cb0\n\
             store i16 %1 @s_cb1\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %1 = load i16 @s_cb0\n\
             call void @1()\n\
             %2 = load i16 @s_cb1\n\
             call void @2()\n\
             ret void\n\
         fn cb0(void) ()\n  block entry:\n    ret void\n\
         fn cb1(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb0_isr"),
        "cb0_isr missing"
    );
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb1_isr"),
        "cb1_isr missing"
    );
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("main call");
    assert_eq!(call.args[0].val, ir::Val::Global("cb0_isr".to_string()));
    assert_eq!(call.args[1].val, ir::Val::Global("cb1_isr".to_string()));
}

/// `llvm.umin`/`llvm.umax` (unsized clamps, e.g. the sd-card byte-shift
/// masks) lower to the same icmp/select tree as their signed cousins,
/// with unsigned predicates (epic-cc#143).
#[test]
fn lowers_umin_umax_to_icmp_select() {
    let m = parse(
        "global a i16\n\
         global b i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @a\n\
             %b = load i16 @b\n\
             %x = call i16 @llvm.umin.i16(i16 %a, i16 %b)\n\
             %y = call i16 @llvm.umax.i16(i16 %x, i16 %b)\n\
             store i16 %y, @a\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(text.contains("%c0 = icmp ult i16 %a %b"), "{text}");
    assert!(text.contains("%x = select i1 %c0 i16 %a i16 %b"), "{text}");
    assert!(text.contains("%c1 = icmp ugt i16 %x %b"), "{text}");
    assert!(text.contains("%y = select i1 %c1 i16 %x i16 %b"), "{text}");
}

/// Lane D fixpoint idempotence (docs/36): a pass meant to reach a fixpoint
/// must be a no-op the second time it runs on its own output. Legalize
/// lowers mul/div/shift/float ops to runtime calls and injects the routine
/// defs; running it again on its own output must not change anything (a
/// pass that thinks it converged but did not would emit a different second
/// result). The canonical text round-trip is the comparison surface.
#[test]
fn legalize_is_a_fixpoint() {
    // A module exercising the mul/shift lowering, the float lowering, and
    // the fcmp materialization tree (the shapes legalize rewrites).
    let m = parse(
        "global in i16\n\
         global f float\n\
         fn main(void) ()\n\
           block entry:\n\
             %a = load i16 @in\n\
             %m = mul i16 %a, 7\n\
             %v = shl i16 %a, %a\n\
             store i16 %m, @in\n\
             %x = load float @f\n\
             %y = fadd float %x %x\n\
             %c = fcmp olt float %x, %y\n\
             store float %y, @f\n\
             ret void\n",
    );
    let once = legalize(m);
    let once_text = ir::serialize(&once);
    let twice = legalize(once);
    let twice_text = ir::serialize(&twice);
    assert_eq!(
        once_text, twice_text,
        "legalize must be a fixpoint (second run is a no-op)"
    );
}

/// Priority duplication (epic-cc#346): main + a high ISR + a low ISR all
/// calling `helper`. The module gains BOTH `helper_isr` (low) and
/// `helper_isr_high` (high); each ISR's call targets its own priority's
/// copy while main's call stays on the original.
#[test]
fn duplicates_shared_functions_per_priority() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn hi(void) [isr] [irq1] ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn lo(void) [isr] [irq2] ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"helper"), "helper must remain: {names:?}");
    assert!(
        names.contains(&"helper_isr"),
        "helper_isr must be added: {names:?}"
    );
    assert!(
        names.contains(&"helper_isr_high"),
        "helper_isr_high must be added: {names:?}"
    );
    // Neither copy is a vector entry.
    assert!(!func("helper_isr", &m2).isr);
    assert!(!func("helper_isr_high", &m2).isr);
    // Each context calls its own copy.
    assert_eq!(call_targets(func("hi", &m2)), ["helper_isr_high"]);
    assert_eq!(call_targets(func("lo", &m2)), ["helper_isr"]);
    assert_eq!(call_targets(func("main", &m2)), ["helper"]);
}

/// A compatibility-mode ISR (priority 0) must not mix with an
/// explicit-priority one: no wiring serves that combination.
#[test]
#[should_panic(expected = "cannot mix with explicit-priority ISRs")]
fn compat_and_explicit_isrs_panic_loudly() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             ret void\n\
         fn a(void) [isr] ()\n\
           block entry:\n\
             ret void\n\
         fn b(void) [isr] [irq1] ()\n\
           block entry:\n\
             ret void\n",
    );
    let _ = legalize(m);
}

/// Two handlers on the same vector have no sound wiring.
#[test]
#[should_panic(expected = "two high-priority interrupt handlers")]
fn duplicate_high_isrs_panic_loudly() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             ret void\n\
         fn a(void) [isr] [irq1] ()\n\
           block entry:\n\
             ret void\n\
         fn b(void) [isr] [irq1] ()\n\
           block entry:\n\
             ret void\n",
    );
    let _ = legalize(m);
}

/// A runtime routine shared with the high context gets an `_isr_high`
/// copy (injected by base name, not by a single-suffix strip): main and
/// the high ISR multiplying means `__mul_u8` plus `__mul_u8_isr_high`,
/// each context calling its own.
#[test]
fn duplicates_shared_runtime_routines_for_the_high_isr() {
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
         fn hi(void) [isr] [irq1] ()\n\
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
        names.contains(&"__mul_u8_isr_high"),
        "the high ISR needs its own routine copy: {names:?}"
    );
    assert_eq!(call_targets(func("hi", &m2)), ["__mul_u8_isr_high"]);
    assert_eq!(call_targets(func("main", &m2)), ["__mul_u8"]);
    assert!(!func("__mul_u8_isr_high", &m2).isr);
}

/// A callback stored into a storage global both contexts dispatch through
/// (the HAL `g_usart->RxCpltCallback(data)` shape with a polled main): the
/// store rewrite puts the `_isr` copy in that shared storage, so MAIN's
/// dispatch site must list the copy too; the strict per-context filter
/// alone would empty the main site and lower it to a trap (epic-cc#568).
#[test]
fn main_site_lists_stored_copy_for_shared_dispatch_storage() {
    let m = parse(
        "global g_storage i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g_storage +2\n\
             store i16 @cb %p\n\
             %q = gep @g_storage +2\n\
             %1 = load i16 %q\n\
             call void @1()\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %r = gep @g_storage +2\n\
             %2 = load i16 %r\n\
             call void @2()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing: the shared-storage callback was not classified"
    );
    let sites = |fname: &str| {
        m2.funcs
            .iter()
            .find(|f| f.name == fname)
            .unwrap()
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Call(c) if !c.callees.is_empty() => Some(c.callees.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sites("main"),
        vec![vec!["cb_isr".to_string()]],
        "main dispatches the shared storage, so its site must list the copy"
    );
    assert_eq!(
        sites("isr"),
        vec![vec!["cb_isr".to_string()]],
        "the ISR site must dispatch the copy"
    );
}

/// The decimal div-rem expansion clang emits for `v % 10` computes its
/// remainder through a `mul i16 q, 246` whose high half is discarded by a
/// `trunc i16 to i8`. Modular arithmetic lets the whole tail run at i8, so
/// the 19-word `__mul_u16` becomes the 5-word `__mul_u8` and the argument
/// staging halves (epic-cc#622).
#[test]
fn narrows_a_low_byte_only_div_rem_tail_to_i8() {
    let m = parse(
        "global in i16\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i16 @in\n\
             %q = udiv i16 %v, 10\n\
             %m = mul i16 %q, 246\n\
             %s = add i16 %m, %v\n\
             %r = trunc i16 %s to i8\n\
             store i8 %r @out\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    // The surviving multiply is the 8-bit routine, and the 16-bit one is
    // not injected at all.
    assert!(
        text.contains("@__mul_u8("),
        "the narrowed tail must call __mul_u8:\n{text}"
    );
    assert!(
        !text.contains("@__mul_u16("),
        "the 16-bit multiply must be gone:\n{text}"
    );
    assert!(
        text.contains("@__udiv_u16("),
        "the divide itself is untouched:\n{text}"
    );
    // The i8 sum carries the trunc's own dst, so the consumer reads the
    // same value it did before.
    assert!(
        text.contains("%r = add i8 "),
        "the sum must be the i8 result under the trunc's name:\n{text}"
    );
}

/// A second use of the multiply or the sum keeps the i16 form: the
/// narrowing is only sound when the low byte is the whole observable
/// result.
#[test]
fn keeps_the_i16_tail_when_the_product_has_another_use() {
    let m = parse(
        "global in i16\n\
         global out i8\n\
         global wide i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i16 @in\n\
             %q = udiv i16 %v, 10\n\
             %m = mul i16 %q, 246\n\
             %s = add i16 %m, %v\n\
             %r = trunc i16 %s to i8\n\
             store i8 %r @out\n\
             store i16 %s @wide\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__mul_u16("),
        "a sum with a 16-bit consumer must keep the wide multiply:\n{text}"
    );
}

/// The narrowed tail needs a register addend: the rewrite emits a `trunc`
/// of it, and isel rejects a const source. A literal addend keeps the wide
/// form rather than producing IR that cannot be selected.
#[test]
fn keeps_the_i16_tail_when_the_addend_is_a_literal() {
    let m = parse(
        "global in i16\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i16 @in\n\
             %q = udiv i16 %v, 10\n\
             %m = mul i16 %q, 246\n\
             %s = add i16 %m, 7\n\
             %r = trunc i16 %s to i8\n\
             store i8 %r @out\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__mul_u16("),
        "a literal addend must keep the wide multiply:\n{text}"
    );
}

/// A callback stored through a loaded handle pointer (`g_handle =
/// &g_storage`, then `%o = load @g_handle; store @cb (gep %o)`) into an
/// ISR-read storage rewrites to the `_isr` copy and the ISR site scopes
/// to it, exactly like the direct-store shape. Before the fix the store
/// kept the main-frame address while the ISR site dispatched the copy,
/// so the compare chain trapped at runtime (epic-cc#642).
#[test]
fn rewrites_callback_stored_through_loaded_handle() {
    let m = parse(
        "global g_storage i16\n\
         global g_handle i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store i16 @g_storage @g_handle\n\
             %o = load i16 @g_handle\n\
             %p = gep %o +0\n\
             store i16 @cb %p\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %o2 = load i16 @g_handle\n\
             %p2 = gep %o2 +0\n\
             %fp = load i16 %p2\n\
             call void @fp()\n\
             ret void\n\
         fn cb(void) ()\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    assert!(
        m2.funcs.iter().any(|f| f.name == "cb_isr"),
        "cb_isr missing"
    );
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let stores: Vec<_> = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| match i {
            Inst::Store(s) => Some(s),
            _ => None,
        })
        .collect();
    let cb_store = stores
        .iter()
        .find(|s| matches!(&s.val, ir::Val::Global(v) if v == "cb" || v == "cb_isr"));
    assert_eq!(
        cb_store.map(|s| &s.val),
        Some(&ir::Val::Global("cb_isr".to_string())),
        "store through the loaded handle must point at the _isr copy"
    );
    let isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    let call = isr
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("isr call");
    assert_eq!(call.callees, vec!["cb_isr".to_string()]);
}

#[test]
fn delay_call_collects_no_callees() {
    // `_delay` is declared, never defined: without the exemption it would
    // collect `cb` (same arity and width) as an indirect candidate and
    // the backends would refuse the call. isel expands it inline instead.
    let m = parse(
        "global g i16\n\
         fn main(void) ()\n\
           block entry:\n\
             store i16 @cb @g\n\
             call void @_delay(i32 100)\n\
             ret void\n\
         fn cb(i32) (0=i32)\n  block entry:\n    ret void\n",
    );
    let m2 = legalize(m);
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            ir::Inst::Call(c) if c.func == "_delay" => Some(c),
            _ => None,
        })
        .expect("_delay call");
    assert!(call.callees.is_empty(), "_delay must keep no callees");
}

/// EC++ function-local statics (#458): the trivialized guard sequence is
/// not re-entrant, so a guard reachable from ISR context panics loudly
/// instead of double-initializing. Here the ISR calls the guarded helper
/// transitively.
#[test]
#[should_panic(expected = "init-once guard cannot be trusted")]
fn isr_context_touching_guard_panics_loudly() {
    let m = parse(
        "global _ZGVg i8\n\
         fn main(void) ()\n\
           block entry:\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             %t = load i8 @_ZGVg\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             call void @helper()\n\
             ret void\n",
    );
    let _ = legalize(m);
}

/// The same guard touched from the main context only is fine, even with
/// an unrelated ISR present.
#[test]
fn main_context_guard_touch_passes() {
    let m = parse(
        "global _ZGVg i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %t = load i8 @_ZGVg\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             ret void\n",
    );
    let m2 = legalize(m);
    assert!(m2.funcs.iter().any(|f| f.name == "main"));
}

/// RAII preservation (#459): destructor calls on every exit path survive
/// the pass unchanged in count and target. Void callees never qualify
/// for call-site sinking, and with no ISR present no copy exists to
/// duplicate into.
#[test]
fn cleanup_calls_survive_legalize_unchanged() {
    let m = parse(
        "fn dtor(i8) (0=i8)\n\
           block entry:\n\
             ret void\n\
         fn work(i8) (0=i8)\n\
           block entry:\n\
             call void @dtor(i8 %0)\n\
             call void @dtor(i8 %0)\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @work(i8 1)\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let work = m2.funcs.iter().find(|f| f.name == "work").expect("work");
    let calls = work
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i, Inst::Call(c) if c.func == "dtor"))
        .count();
    assert_eq!(calls, 2, "both cleanup calls must survive");
    assert!(
        m2.funcs.iter().any(|f| f.name == "dtor"),
        "called dtor def must stay"
    );
}

/// A vtable slot ref (pos, target, addend) feeds the address-taken set
/// through the const global, not an instruction: the indirect dispatch
/// call collects the slot target as a callee (epic-cc#460). The canonical
/// text carries no refs, so the test injects the decoded shape directly.
#[test]
fn fills_indirect_callees_from_vtable_refs() {
    let mut m = parse(
        "global vt i8\n\
         fn tick(i8) (0=i8)\n  block entry:\n    ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %f = load i8 @vt\n\
             call void @1(i8 %f)\n\
             ret void\n",
    );
    for g in &mut m.globals {
        if g.name == "vt" {
            g.refs = vec![(4, "tick".to_string(), 0)];
        }
    }
    let m2 = legalize(m);
    let main = m2.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            ir::Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("indirect call");
    assert_eq!(call.callees, vec!["tick".to_string()]);
}

/// epic-cc#722: a u32 decimal-digit loop (udiv-10 / mul-246 / truncating add
/// / or-48 / indexed byte-store / count bump / ult-10 single-block do-while)
/// becomes one `__udec_u32` call on the PIC18 entry, with the routine Func
/// injected; the plain entry keeps the expanded loop. Sharing needs at
/// least two sites: one lone loop stays expanded, since a call plus the
/// helper body cost more than the inline loop. Near-misses (volatile store,
/// a different divisor, a `ule` exit, a second outside use of the quotient)
/// never count as sites.
#[test]
fn shares_decimal_digit_loops_on_pic18_only() {
    use legalize::legalize_pic18;
    let loop_fn = |name: &str, tail: &str| {
        format!(
            "fn {name}(void) ()\n  block entry:\n    %v0 = load i32 @vin\n    br loop\n  block loop:\n    %cur = phi i32 %q loop %v0 entry\n    %n = phi i8 %n1 loop 0 entry\n    %q = udiv i32 %cur, 10\n    %m = mul i32 %q, 246\n    %s = add i32 %m, %cur\n    %r = trunc i32 %s to i8\n    %d = or i8 %r, 48\n    %ix = zext i8 %n to i16\n    %p = gep @buf +0 +1*%ix\n    {tail}\n    %n1 = add i8 %n, 1\n    %c = icmp ult i32 %cur, 10\n    br i1 %c exit loop\n  block exit:\n    ret void\n"
        )
    };
    let good = |name: &str| loop_fn(name, "store i8 %d %p");
    let header = "global buf i8\nglobal vin i32\n";
    let pair = format!("{header}{}", good("emit1")) + &good("emit2");
    let m = legalize_pic18(parse(&pair));
    let text = ir::serialize(&m);
    assert_eq!(
        text.matches("call i8 @__udec_u32(i32 %v0, @buf)").count(),
        2,
        "both loops become helper calls:\n{text}"
    );
    assert!(
        !text.contains("udiv i32"),
        "no expanded divide remains:\n{text}"
    );
    let helper = m
        .funcs
        .iter()
        .find(|f| f.name == "__udec_u32")
        .expect("routine injected");
    assert_eq!(helper.ret, Some(ir::Ty::I8));
    assert_eq!(helper.params.len(), 2);
    assert_eq!(helper.params[0].name, "num");
    assert_eq!(helper.params[0].width, 4);
    assert!(helper.params[1].ptr);
    assert_eq!(helper.params[1].width, 2);
    // One lone loop stays expanded: sharing would grow.
    let lone = format!("{header}{}", good("emit1"));
    let out = ir::serialize(&legalize_pic18(parse(&lone)));
    assert!(
        out.contains("__udiv_u32") && !out.contains("__udec_u32"),
        "lone loop stays expanded:\n{out}"
    );
    // The plain entry keeps both expanded loops and injects nothing.
    let plain = ir::serialize(&legalize(parse(&pair)));
    assert!(
        plain.matches("call i32 @__udiv_u32").count() == 2 && plain.contains("or i8"),
        "plain entry keeps the loops:\n{plain}"
    );
    assert!(
        !plain.contains("__udec_u32"),
        "plain entry injects nothing:\n{plain}"
    );
    // Near-misses: two good loops plus one broken loop still share exactly
    // the two good ones; the broken loop stays expanded.
    let broken_tail = loop_fn("emit3", "store volatile i8 %d %p");
    let broken_div = good("emit3").replace("udiv i32 %cur, 10", "udiv i32 %cur, 11");
    let broken_exit = good("emit3").replace("icmp ult i32 %cur, 10", "icmp ule i32 %cur, 10");
    let broken_swap = good("emit3").replace("icmp ult i32 %cur, 10", "icmp ult i32 10, %cur");
    let broken_leak = good("emit3").replace(
        "block exit:\n    ret void",
        "block exit:\n    %leak = add i32 %q, %v0\n    store i32 %leak @vin\n    ret void",
    );
    for (name, bad) in [
        ("volatile", broken_tail),
        ("divisor", broken_div),
        ("exit", broken_exit),
        ("swap", broken_swap),
        ("leak", broken_leak),
    ] {
        let src = format!("{header}{}{}", good("emit1"), good("emit2")) + &bad;
        let out = ir::serialize(&legalize_pic18(parse(&src)));
        assert_eq!(
            out.matches("call i8 @__udec_u32(i32 %v0, @buf)").count(),
            2,
            "{name}: the two good loops still share:\n{out}"
        );
        assert!(
            out.contains("__udiv_u32"),
            "{name}: the broken loop stays expanded:\n{out}"
        );
    }
}

/// epic-cc#722: the bench-u16-dec counted loop (i16 counter phi, udiv-10 /
/// mul-246 / truncating add, indexed byte store, count bump, `eq`-on-count
/// exit after 5 digits) becomes one void `__udec_u16_5` call on the PIC18
/// entry, with the routine Func injected; the plain entry keeps the
/// expanded loop. Unlike the do-while sharing this fires on a single site.
#[test]
fn shares_bench_counted_loop_on_pic18_only() {
    use legalize::legalize_pic18;
    let base = "global digits i8\nglobal in i16\nfn main(void) ()\n  block 0:\n    %1 = load volatile i16 @in\n    %2 = freeze i16 %1\n    br 3\n  block 3:\n    %4 = phi i16 0 0 %9 3\n    %5 = phi i16 %2 0 %6 3\n    %6 = udiv i16 %5 10\n    %.neg = mul i16 %6 246\n    %7 = add i16 %.neg %5\n    %8 = trunc i16 %7 to i8\n    %scevgep = gep @digits +0 +1*%4\n    store volatile i8 %8 %scevgep\n    %9 = add i16 %4 1\n    %10 = icmp eq i16 %9 5\n    br i1 %10 11 3\n  block 11:\n    ret void\n";
    let m = legalize_pic18(parse(base));
    let text = ir::serialize(&m);
    assert!(
        text.contains("call void @__udec_u16_5(i16 %2, @digits)"),
        "counted loop becomes one helper call:\n{text}"
    );
    assert!(
        !text.contains("udiv i16"),
        "no expanded divide remains:\n{text}"
    );
    let helper = m
        .funcs
        .iter()
        .find(|f| f.name == "__udec_u16_5")
        .expect("routine injected");
    assert_eq!(helper.ret, None);
    assert_eq!(helper.params.len(), 2);
    assert_eq!(helper.params[0].name, "num");
    assert_eq!(helper.params[0].width, 2);
    assert!(helper.params[1].ptr);
    // The plain entry keeps the expanded loop and injects nothing.
    let plain = ir::serialize(&legalize(parse(base)));
    assert!(
        plain.contains("__udiv_u16") && !plain.contains("__udec_u16_5"),
        "plain entry keeps the loop:\n{plain}"
    );
    // Near-misses keep the loop: another divisor, another trip count, an
    // ult-on-value exit, a leaked quotient.
    for (name, src) in [
        ("divisor", base.replace("udiv i16 %5 10", "udiv i16 %5 11")),
        ("trip", base.replace("icmp eq i16 %9 5", "icmp eq i16 %9 6")),
        (
            "exit",
            base.replace("icmp eq i16 %9 5", "icmp ult i16 %5 10"),
        ),
        (
            "leak",
            base.replace(
                "block 11:\n    ret void",
                "block 11:\n    %leak = add i16 %6 %2\n    store i16 %leak @in\n    ret void",
            ),
        ),
    ] {
        let out = ir::serialize(&legalize_pic18(parse(&src)));
        assert!(
            out.contains("__udiv_u16") && !out.contains("__udec_u16_5"),
            "{name} keeps the loop:\n{out}"
        );
    }
}

/// A same-block `udiv`/`urem` pair on identical operands fuses into one
/// combined divide call plus a load of its remainder spill slot
/// (epic-cc#895): the loop runs once instead of twice.
#[test]
fn fuses_matching_divmod_pair() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             %6 = urem i16 %1 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("%3 = call i16 @__udivmod_u16(i16 %1, i16 %2)"),
        "divide became the combined call:\n{text}"
    );
    assert!(
        text.contains("%6 = load volatile i16 @__udivmod_rem_u16"),
        "remainder became a slot load:\n{text}"
    );
    assert!(
        text.contains("global __udivmod_rem_u16 i16"),
        "spill slot injected:\n{text}"
    );
    assert!(
        text.contains("fn __udivmod_u16(i16) (num=i16, den=i16)"),
        "combined routine injected:\n{text}"
    );
    assert!(
        !text.contains("fn __udiv_u16(") && !text.contains("fn __urem_u16("),
        "pruned routines not injected:\n{text}"
    );
}

/// Reloaded volatile globals never fuse: two reads of one volatile global
/// may return different values (MMIO, or an ISR-shared flag changed
/// between them), so the second pair is not provably the first.
#[test]
fn no_fuse_on_volatile_reloads() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load volatile i16 @a\n\
             %2 = load volatile i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store volatile i16 %3 @q\n\
             %4 = load volatile i16 @a\n\
             %5 = load volatile i16 @b\n\
             %6 = urem i16 %4 %5\n\
             store volatile i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udiv_u16(") && text.contains("@__urem_u16("),
        "volatile pair kept two calls:\n{text}"
    );
    assert!(!text.contains("__udivmod"), "volatile pair fused:\n{text}");
}

/// Reloaded non-volatile globals fuse across an unrelated store: plain
/// memory changes only through visible stores and calls.
#[test]
fn fuses_nonvolatile_reloads_across_unrelated_store() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             %4 = load i16 @a\n\
             %5 = load i16 @b\n\
             %6 = urem i16 %4 %5\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udivmod_u16(i16 %1, i16 %2)")
            && text.contains("%6 = load volatile i16 @__udivmod_rem_u16"),
        "reloaded pair fused:\n{text}"
    );
}

/// One volatile load pair shared by both operations fuses: a single read
/// has one value, however it was obtained.
#[test]
fn fuses_volatile_shared_operands() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load volatile i16 @a\n\
             %2 = load volatile i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store volatile i16 %3 @q\n\
             %6 = urem i16 %1 %2\n\
             store volatile i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udivmod_u16(i16 %1, i16 %2)")
            && text.contains("%6 = load volatile i16 @__udivmod_rem_u16"),
        "shared-operand pair fused:\n{text}"
    );
}

/// A store to an operand global between the operations blocks the fuse:
/// the second load may read a new value.
#[test]
fn no_fuse_on_store_to_operand() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             store i16 %3 @a\n\
             %4 = load i16 @a\n\
             %6 = urem i16 %4 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udiv_u16(") && text.contains("@__urem_u16("),
        "pair kept two calls:\n{text}"
    );
    assert!(!text.contains("__udivmod"), "nothing fused:\n{text}");
}

/// A call between the operations blocks the fuse: it may store to the
/// operand globals, and a second fused pair would clobber the slot.
#[test]
fn no_fuse_on_call_between() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn helper(i16) (x=i16)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             call void @helper(i16 %1)\n\
             %6 = urem i16 %1 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udiv_u16(") && text.contains("@__urem_u16("),
        "pair kept two calls:\n{text}"
    );
    assert!(!text.contains("__udivmod"), "nothing fused:\n{text}");
}

/// Mismatched operands, widths, or order never fuse.
#[test]
fn no_fuse_on_mismatch_or_reverse_order() {
    for (name, body) in [
        (
            "denominator",
            "%3 = udiv i16 %1 %2\n             store i16 %3 @q\n             %6 = urem i16 %1 %1\n",
        ),
        (
            "width",
            "%3 = udiv i16 %1 %2\n             store i16 %3 @q\n             %8 = load i8 @qb\n             %6 = urem i8 %8 %8\n",
        ),
        (
            "reverse",
            "%6 = urem i16 %1 %2\n             store i16 %6 @m\n             %3 = udiv i16 %1 %2\n",
        ),
    ] {
        let m = parse(&format!(
            "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\nglobal qb i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %1 = load i16 @a\n\
                 %2 = load i16 @b\n\
                 {body}             store i16 %6 @m\n\
                 ret void\n",
        ));
        let text = ir::serialize(&legalize(m));
        assert!(!text.contains("__udivmod"), "{name} fused:\n{text}");
    }
}

/// Interrupt-context spellings keep their two calls: each context needs
/// its own remainder slot, and fusion only provides the main one.
#[test]
fn no_fuse_on_isr_spellings() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = call i16 @__udiv_u16_isr(i16 %1, i16 %2)\n\
             store i16 %3 @q\n\
             %6 = call i16 @__urem_u16_isr(i16 %1, i16 %2)\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(!text.contains("__udivmod"), "isr pair fused:\n{text}");
}

/// A second remainder below the first fuses onto the same combined call.
#[test]
fn fuses_chained_remainders() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\nglobal m2 i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             %6 = urem i16 %1 %2\n\
             store i16 %6 @m\n\
             %7 = urem i16 %1 %2\n\
             store i16 %7 @m2\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        text.contains("@__udivmod_u16(")
            && text.contains("%6 = load volatile i16 @__udivmod_rem_u16")
            && text.contains("%7 = load volatile i16 @__udivmod_rem_u16"),
        "both remainders fused:\n{text}"
    );
}

/// A remainder slot shadowed by any user global never fuses, whatever its
/// type: reusing the name would alias the user's variable.
#[test]
fn no_fuse_on_slot_collision() {
    for ty in ["i8", "i16"] {
        let m = parse(&format!(
            "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\nglobal __udivmod_rem_u16 {ty}\n\
             fn main(void) ()\n\
               block entry:\n\
                 %1 = load i16 @a\n\
                 %2 = load i16 @b\n\
                 %3 = udiv i16 %1 %2\n\
                 store i16 %3 @q\n\
                 %6 = urem i16 %1 %2\n\
                 store i16 %6 @m\n\
                 ret void\n",
        ));
        let text = ir::serialize(&legalize(m));
        assert!(
            !text.contains("__udivmod_u16"),
            "{ty}-typed collision fused:\n{text}"
        );
    }
}

/// A store to an operand global between the two proving loads blocks the
/// fuse even though the calls themselves are adjacent.
#[test]
fn no_fuse_on_store_between_loads() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             store i16 %1 @a\n\
             %4 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             %6 = urem i16 %4 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(!text.contains("__udivmod"), "stale load fused:\n{text}");
}

/// An indirect store or a memcpy between the operations blocks the fuse:
/// either may write an operand global through an untracked address.
#[test]
fn no_fuse_on_opaque_writes_between() {
    for (name, middle) in [
        ("indirect store", "store i16 %2 %p\n"),
        ("memcpy", "memcpy @m %1 2\n"),
    ] {
        let m = parse(&format!(
            "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
             fn main(void) ()\n\
               block entry:\n\
                 %1 = load i16 @a\n\
                 %2 = load i16 @b\n\
                 %3 = udiv i16 %1 %2\n\
                 store i16 %3 @q\n\
                 {middle}             %6 = urem i16 %1 %2\n\
                 store i16 %6 @m\n\
                 ret void\n",
        ));
        let text = ir::serialize(&legalize(m));
        assert!(!text.contains("__udivmod"), "{name} fused:\n{text}");
    }
}

/// A pair split across blocks never fuses: matching is same-block only.
#[test]
fn no_fuse_across_blocks() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             br next\n\
           block next:\n\
             %6 = urem i16 %1 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize(m));
    assert!(
        !text.contains("__udivmod"),
        "cross-block pair fused:\n{text}"
    );
}

/// The PIC18 entry fuses like the main one now that isel-pic18 owns
/// combined recipes (epic-cc#982).
#[test]
fn fuses_matching_pair_on_pic18_entry() {
    use legalize::legalize_pic18;
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = udiv i16 %1 %2\n\
             store i16 %3 @q\n\
             %6 = urem i16 %1 %2\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize_pic18(m));
    assert!(
        text.contains("@__udivmod_u16(")
            && text.contains("%6 = load volatile i16 @__udivmod_rem_u16"),
        "pic18 pair kept two calls:\n{text}"
    );
}

/// ISR spellings keep two calls on the PIC18 entry too: fusion only
/// provides the main-context slot.
#[test]
fn no_fuse_on_pic18_isr_spellings() {
    use legalize::legalize_pic18;
    let m = parse(
        "global a i16\nglobal b i16\nglobal q i16\nglobal m i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = load i16 @b\n\
             %3 = call i16 @__udiv_u16_isr(i16 %1, i16 %2)\n\
             store i16 %3 @q\n\
             %6 = call i16 @__urem_u16_isr(i16 %1, i16 %2)\n\
             store i16 %6 @m\n\
             ret void\n",
    );
    let text = ir::serialize(&legalize_pic18(m));
    assert!(!text.contains("__udivmod"), "isr pair fused:\n{text}");
}
