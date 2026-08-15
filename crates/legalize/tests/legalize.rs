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
