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
