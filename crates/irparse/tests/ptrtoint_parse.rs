use ir::{Inst, Ty, Val};
use irparse::parse_ll;

// epic-cc#1422: a `ptrtoint` constant expression in value position
// (`(uintptr_t)tab` on a global) reaches irparse as an operand, not as
// an instruction. It must lower to the same value the space proof
// tracks: an integer copy of the pointer (a same-width `trunc` over a
// materialized GEP, or the global itself for a plain address).

#[test]
fn store_ptrtoint_of_global_lowers_to_the_global() {
    let m = parse_ll(
        "@tab = constant [4 x i8] c\"\\11\\22\\33\\44\", align 1\n\
         @laundered = global i16 0, align 2\n\
         define void @main() {\n\
         \x20 store volatile i16 ptrtoint (ptr @tab to i16), ptr @laundered, align 2\n\
         \x20 ret void\n\
         }\n",
    );
    let body = &m.funcs[0].blocks[0].insts;
    match &body[0] {
        Inst::Store(s) => {
            assert_eq!(s.ty, Ty::I16);
            assert_eq!(s.val, Val::Global("tab".to_string()));
            assert_eq!(s.ptr, "@laundered");
        }
        other => panic!("expected Store, got {other:?}"),
    }
}

#[test]
fn store_ptrtoint_of_gep_materializes_a_trunc_copy() {
    let m = parse_ll(
        "@tab = constant [4 x i8] c\"\\11\\22\\33\\44\", align 1\n\
         @laundered = global i16 0, align 2\n\
         define void @main() {\n\
         \x20 store volatile i16 ptrtoint (ptr getelementptr inbounds ([4 x i8], ptr @tab, i16 0, i16 2) to i16), ptr @laundered, align 2\n\
         \x20 ret void\n\
         }\n",
    );
    let body = &m.funcs[0].blocks[0].insts;
    // A Gep materializes the element address, a same-width Trunc copies
    // it into the integer domain, and the Store carries that register.
    assert!(
        matches!(body[0], Inst::Gep(_)),
        "expected Gep, got {:?}",
        body[0]
    );
    match &body[1] {
        Inst::Trunc(t) => {
            assert_eq!(t.from, Ty::I16);
            assert_eq!(t.to, Ty::I16);
        }
        other => panic!("expected Trunc, got {other:?}"),
    }
    match &body[2] {
        Inst::Store(s) => assert!(matches!(s.val, Val::Reg(_))),
        other => panic!("expected Store, got {other:?}"),
    }
}

#[test]
fn call_arg_ptrtoint_of_global_lowers_to_the_global() {
    let m = parse_ll(
        "@tab = constant [4 x i8] c\"\\11\\22\\33\\44\", align 1\n\
         declare i16 @id(i16)\n\
         define i16 @main() {\n\
         \x20 %r = call i16 @id(i16 ptrtoint (ptr @tab to i16))\n\
         \x20 ret i16 %r\n\
         }\n",
    );
    let body = &m.funcs[0].blocks[0].insts;
    match &body[0] {
        Inst::Call(c) => {
            assert_eq!(c.func, "id");
            assert_eq!(c.ty, Some(Ty::I16));
            assert_eq!(c.args.len(), 1);
            assert_eq!(c.args[0].val, Val::Global("tab".to_string()));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

#[test]
fn call_with_multi_type_signature_parses() {
    let m = parse_ll(
        "declare i16 @f(i16)\n\
         define i16 @main(i16 %x) {\n\
         \x20 %r = call i16 @f(i16 %x)\n\
         \x20 ret i16 %r\n\
         }\n",
    );
    let body = &m.funcs[0].blocks[0].insts;
    match &body[0] {
        Inst::Call(c) => {
            assert_eq!(c.func, "f");
            assert_eq!(c.ty, Some(Ty::I16));
            assert_eq!(c.args.len(), 1);
            assert_eq!(c.args[0].ty, Some(Ty::I16));
            assert_eq!(c.args[0].val, Val::Reg("x".to_string()));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}
