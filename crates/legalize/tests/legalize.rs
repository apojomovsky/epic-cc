use legalize::legalize;
use ir::{parse, Inst};

#[test]
fn passes_8_bit_through() {
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    ret void\n");
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
    assert!(text.contains("fn __mul_u16(i16) (a=i16, b=i16)\n  block entry:\n    %__scr = alloca 14"));
    assert!(text.contains("fn __shl_u16(i16) (val=i16, cnt=i16)\n  block entry:\n    %__scr = alloca 4"));
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
    let out = alloc::allocate(&m2, "edge main __mul_u16\nedge main __shl_u16\n");
    assert_eq!(out.locals["__mul_u16::b"], out.locals["__mul_u16::a"] + 2);
    assert_eq!(out.locals["__mul_u16::__scr"], out.locals["__mul_u16::b"] + 2);
    assert_eq!(out.locals["__shl_u16::cnt"], out.locals["__shl_u16::val"] + 2);
    assert_eq!(out.locals["__shl_u16::__scr"], out.locals["__shl_u16::cnt"] + 2);
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
        ("mul",  "i8",  Ty::I8,  "__mul_u8",  &["a", "b"], 6),
        ("mul",  "i16", Ty::I16, "__mul_u16", &["a", "b"], 14),
        ("udiv", "i8",  Ty::I8,  "__udiv_u8",  &["num", "den"], 4),
        ("udiv", "i16", Ty::I16, "__udiv_u16", &["num", "den"], 7),
        ("urem", "i8",  Ty::I8,  "__urem_u8",  &["num", "den"], 4),
        ("urem", "i16", Ty::I16, "__urem_u16", &["num", "den"], 7),
        ("sdiv", "i8",  Ty::I8,  "__sdiv_i8",  &["num", "den"], 5),
        ("sdiv", "i16", Ty::I16, "__sdiv_i16", &["num", "den"], 7),
        ("srem", "i8",  Ty::I8,  "__srem_i8",  &["num", "den"], 5),
        ("srem", "i16", Ty::I16, "__srem_i16", &["num", "den"], 7),
        ("shl",  "i8",  Ty::I8,  "__shl_u8",   &["val", "cnt"], 3),
        ("shl",  "i16", Ty::I16, "__shl_u16",  &["val", "cnt"], 4),
        ("lshr", "i8",  Ty::I8,  "__lshr_u8",  &["val", "cnt"], 3),
        ("lshr", "i16", Ty::I16, "__lshr_u16", &["val", "cnt"], 4),
        ("ashr", "i8",  Ty::I8,  "__ashr_i8",  &["val", "cnt"], 3),
        ("ashr", "i16", Ty::I16, "__ashr_i16", &["val", "cnt"], 4),
        ("mul",  "i32", Ty::I32, "__mul_u32",  &["a", "b"], 11),
        ("udiv", "i32", Ty::I32, "__udiv_u32", &["num", "den"], 10),
        ("urem", "i32", Ty::I32, "__urem_u32", &["num", "den"], 10),
        ("sdiv", "i32", Ty::I32, "__sdiv_i32", &["num", "den"], 12),
        ("srem", "i32", Ty::I32, "__srem_i32", &["num", "den"], 12),
        ("shl",  "i32", Ty::I32, "__shl_u32",  &["val", "cnt"], 2),
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
            assert_eq!(f.params[i].width, ty_enum.bytes(), "{routine} param {i} width");
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
             %h = add i8 1, 2\n\
             ret void\n",
    );
    let m2 = legalize(m);
    let names: Vec<&str> = m2.funcs.iter().map(|f| f.name.as_str()).collect();
    // Both the original and the _isr copy exist.
    assert!(names.contains(&"helper"), "helper must remain: {names:?}");
    assert!(names.contains(&"helper_isr"), "helper_isr must be added: {names:?}");
    // The copy is a deep clone: same body, `isr` flag cleared.
    let helper_isr = func("helper_isr", &m2);
    assert!(!helper_isr.isr, "the _isr copy must not be marked isr");
    match &helper_isr.blocks[0].insts[0] {
        Inst::Bin(b) => assert_eq!(b.dst, "h", "the copy must carry helper's body"),
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
    assert!(names.contains(&"helper_isr"), "helper_isr missing: {names:?}");
    assert!(names.contains(&"helper2_isr"), "helper2_isr missing: {names:?}");
    assert!(names.contains(&"isr_only"), "isr_only must remain: {names:?}");
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
    assert!(text.contains("fn helper_isr(void) ()"), "missing copy:\n{text}");
    assert!(text.contains("fn isr(void) [isr] ()"), "isr marker lost:\n{text}");
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
    assert!(!text.contains("_isr"), "no ISR: must not duplicate:\n{text}");
    assert_eq!(text, ir::serialize(&parse(src)), "no ISR: must be byte-identical");
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
    assert!(!names.contains(&"helper_isr"), "helper is not shared: {names:?}");
    assert!(!names.contains(&"isr_private_isr"), "isr_private is not shared: {names:?}");
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
