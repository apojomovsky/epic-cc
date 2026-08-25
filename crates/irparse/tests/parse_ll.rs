use ir::{CallArg, GepBase, Global, Inst, MemLen, Ty, Val};
use irparse::parse_ll;

// Array/`constant` globals + getelementptr (phase-3 pointers/const).
const GEP_ARRAY: &str = r#"
@ram = dso_local global [8 x i8] zeroinitializer, align 1
@table = dso_local constant [4 x i8] c"\0A\14\1E(", align 1
define dso_local void @main() {
  %3 = add nsw i16 0, 1
  %p = getelementptr i8, ptr @ram, i16 %3
  %q = getelementptr [4 x i8], ptr @table, i16 0, i16 %3
  ret void
}
"#;

#[test]
fn parses_array_and_const_globals_and_gep() {
    let m = parse_ll(GEP_ARRAY);
    assert_eq!(m.globals.len(), 2);

    // @ram = global [8 x i8] zeroinitializer
    match &m.globals[0] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "ram");
            assert_eq!(*ty, ir::Ty::I8);
            assert!(!is_const);
            assert_eq!(*size, 8);
            assert_eq!(bytes, &vec![0u8; 8]);
            assert_eq!(*addr, None);
        }
    }

    // @table = constant [4 x i8] c"\0A\14\1E("
    match &m.globals[1] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "table");
            assert_eq!(*ty, ir::Ty::I8);
            assert!(is_const);
            assert_eq!(*size, 4);
            assert_eq!(bytes, &vec![0x0A, 0x14, 0x1E, 0x28]);
            assert_eq!(*addr, None);
        }
    }

    let body = &m.funcs[0].blocks[0].insts;
    assert_eq!(body.len(), 4);

    // %p = getelementptr i8, ptr @ram, i16 %3 -> base ram, k 0, term 1*%3
    match &body[1] {
        Inst::Gep(g) => {
            assert_eq!(g.dst, "p");
            assert_eq!(g.base, GepBase::Global("ram".to_string()));
            assert_eq!(g.k, 0);
            assert_eq!(g.terms, vec![(1, "3".to_string())]);
        }
        other => panic!("expected Gep, got {other:?}"),
    }

    // %q = getelementptr [4 x i8], ptr @table, i16 0, i16 %3
    //  -> base table, const 0 folds to k 0, last index %3 * i8 stride = 1*%3
    match &body[2] {
        Inst::Gep(g) => {
            assert_eq!(g.dst, "q");
            assert_eq!(g.base, GepBase::Global("table".to_string()));
            assert_eq!(g.k, 0);
            assert_eq!(g.terms, vec![(1, "3".to_string())]);
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

// LLVM prints a table byte 0x5C (`\`) as the `\\` escape (not `\5C`), so a
// const table spanning the printable range (e.g. bytes 0..255 = 0x00..0xFF)
// contains `\\` in its `c"..."` initializer. The string-literal decoder must
// turn each `\\` back into a single 0x5C byte — the old hex-only decoder
// panicked on the `\` (to_digit(16) -> None).
const BACKSLASH_LITERAL: &str = r#"
@t = dso_local constant [4 x i8] c"\00\\\\\FF", align 1
define dso_local void @main() {
  ret void
}
"#;

#[test]
fn parses_backslash_escapes_in_const_literals() {
    let m = parse_ll(BACKSLASH_LITERAL);
    let g = m.globals.iter().find(|g| g.name == "t").expect("const @t");
    assert!(g.is_const);
    assert_eq!(
        g.bytes,
        vec![0x00, 0x5C, 0x5C, 0xFF],
        "`\\\\` must decode to one 0x5C byte each"
    );
}

const LL: &str = r#"
@in = dso_local global i8 0, align 1
@out = dso_local global i8 0, align 1
define dso_local void @main() {
  %1 = load volatile i8, ptr @in, align 1
  %2 = add nsw i8 %1, 1
  store volatile i8 %2, ptr @out, align 1
  ret void
}
"#;

#[test]
fn parses_straight_line_ll() {
    let m = parse_ll(LL);
    assert_eq!(m.globals.len(), 2);
    assert_eq!(m.funcs.len(), 1);
    assert_eq!(m.funcs[0].blocks.len(), 1);
    assert_eq!(m.funcs[0].blocks[0].insts.len(), 4);
}

// Probe from spike/probe.ll (trimmed): control flow, calls, casts, phi.
const PROBE: &str = r#"
@in = dso_local global i8 0, align 1
@out = dso_local global i8 0, align 1

define dso_local void @main() local_unnamed_addr #0 {
  %1 = load volatile i8, ptr @in, align 1, !tbaa !2
  %2 = zext i8 %1 to i16
  %3 = icmp eq i8 %1, 0
  br i1 %3, label %6, label %8

4:                                                ; preds = %8
  %5 = trunc i16 %14 to i8
  br label %6

6:                                                ; preds = %4, %0
  %7 = phi i8 [ 0, %0 ], [ %5, %4 ]
  store volatile i8 %7, ptr @out, align 1, !tbaa !2
  ret void

8:                                                ; preds = %0, %8
  %9 = phi i16 [ %15, %8 ], [ 0, %0 ]
  %10 = phi i16 [ %14, %8 ], [ 0, %0 ]
  %11 = and i16 %9, 1
  %12 = icmp eq i16 %11, 0
  %13 = select i1 %12, i16 100, i16 %9
  %14 = tail call fastcc i16 @add(i16 noundef %10, i16 noundef %13) #2
  %15 = add nuw nsw i16 %9, 1
  %16 = icmp eq i16 %15, %2
  br i1 %16, label %4, label %8, !llvm.loop !5
}

define internal fastcc i16 @add(i16 noundef %0, i16 noundef range(i16 -32768, 255) %1) unnamed_addr #1 {
  %3 = add nsw i16 %1, %0
  ret i16 %3
}
"#;

#[test]
fn parses_probe_control_flow_calls_and_casts() {
    let m = parse_ll(PROBE);
    assert_eq!(m.funcs.len(), 2);

    let main = &m.funcs[0];
    assert_eq!(main.name, "main");
    let labels: Vec<&str> = main.blocks.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, ["0", "4", "6", "8"]);

    // phi i8 in block 6: incoming (Const 0, "0"), (Reg 5, "4")
    let phi = &main.blocks[2].insts[0];
    match phi {
        Inst::Phi(p) => {
            assert_eq!(p.dst, "7");
            assert_eq!(p.ty, ir::Ty::I8);
            assert_eq!(p.incoming.len(), 2);
            assert_eq!(p.incoming[0], (Val::Const(0), "0".to_string()));
            assert_eq!(p.incoming[1], (Val::Reg("5".to_string()), "4".to_string()));
        }
        other => panic!("expected Phi, got {other:?}"),
    }

    // call i16 @add(i16 %10, i16 %13) in block 8: 2 args in order
    let call = &main.blocks[3].insts[5];
    match call {
        Inst::Call(c) => {
            assert_eq!(c.func, "add");
            assert_eq!(c.ty, Some(ir::Ty::I16));
            assert_eq!(c.args.len(), 2);
            assert_eq!(
                c.args[0],
                CallArg {
                    ty: Some(ir::Ty::I16),
                    val: Val::Reg("10".to_string()),
                    byval: None,
                    sret: false
                }
            );
            assert_eq!(
                c.args[1],
                CallArg {
                    ty: Some(ir::Ty::I16),
                    val: Val::Reg("13".to_string()),
                    byval: None,
                    sret: false
                }
            );
        }
        other => panic!("expected Call, got {other:?}"),
    }

    // br i1 %16, label %4, label %8 in block 8
    let br = &main.blocks[3].insts.last().unwrap();
    match br {
        Inst::BrCond(b) => {
            assert_eq!(b.cond, Val::Reg("16".to_string()));
            assert_eq!(b.t, "4");
            assert_eq!(b.f, "8");
        }
        other => panic!("expected BrCond, got {other:?}"),
    }

    // zext in block 0 and trunc in block 4
    match &main.blocks[0].insts[1] {
        Inst::Zext(z) => {
            assert_eq!(z.dst, "2");
            assert_eq!(z.from, ir::Ty::I8);
            assert_eq!(z.val, Val::Reg("1".to_string()));
            assert_eq!(z.to, ir::Ty::I16);
        }
        other => panic!("expected Zext, got {other:?}"),
    }
    match &main.blocks[1].insts[0] {
        Inst::Trunc(t) => {
            assert_eq!(t.dst, "5");
            assert_eq!(t.from, ir::Ty::I16);
            assert_eq!(t.to, ir::Ty::I8);
        }
        other => panic!("expected Trunc, got {other:?}"),
    }

    // select i1 %12, i16 100, i16 %9
    match &main.blocks[3].insts[4] {
        Inst::Select(s) => {
            assert_eq!(s.cond, Val::Reg("12".to_string()));
            assert_eq!(s.ty, ir::Ty::I16);
            assert_eq!(s.a, Val::Const(100));
            assert_eq!(s.b, Val::Reg("9".to_string()));
        }
        other => panic!("expected Select, got {other:?}"),
    }

    // icmp eq i16 %11, 0
    match &main.blocks[3].insts[3] {
        Inst::Icmp(i) => {
            assert_eq!(i.pred, "eq");
            assert_eq!(i.ty, ir::Ty::I16);
            assert_eq!(i.a, Val::Reg("11".to_string()));
            assert_eq!(i.b, Val::Const(0));
        }
        other => panic!("expected Icmp, got {other:?}"),
    }
}

#[test]
fn parses_all_icmp_predicates_and_sext() {
    let ll = r#"
define i16 @main() {
  %a = sext i8 0 to i16
  %1 = icmp eq i8 0, 1
  %2 = icmp ne i8 0, 1
  %3 = icmp ult i8 0, 1
  %4 = icmp ule i8 0, 1
  %5 = icmp ugt i8 0, 1
  %6 = icmp uge i8 0, 1
  %7 = icmp slt i8 0, 1
  %8 = icmp sle i8 0, 1
  %9 = icmp sgt i8 0, 1
  %10 = icmp sge i8 0, 1
  ret i16 %a
}
"#;
    let m = parse_ll(ll);
    let body = &m.funcs[0].blocks[0].insts;
    assert_eq!(body.len(), 12); // sext + 10 icmps + ret

    match &body[0] {
        Inst::Sext(s) => {
            assert_eq!(s.dst, "a");
            assert_eq!(s.from, ir::Ty::I8);
            assert_eq!(s.val, Val::Const(0));
            assert_eq!(s.to, ir::Ty::I16);
        }
        other => panic!("expected Sext, got {other:?}"),
    }

    for (idx, p) in [
        "eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge",
    ]
    .iter()
    .enumerate()
    {
        match &body[idx + 1] {
            Inst::Icmp(i) => {
                assert_eq!(&i.pred, p);
                assert_eq!(i.ty, ir::Ty::I8);
            }
            other => panic!("expected Icmp, got {other:?}"),
        }
    }
}

#[test]
fn parses_icmp_with_samesign_flag() {
    let ll = r#"
define i8 @main() {
  %1 = icmp samesign ugt i8 5, 2
  ret i8 0
}
"#;
    let m = parse_ll(ll);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Icmp(i) => {
            assert_eq!(i.pred, "ugt");
            assert_eq!(i.ty, ir::Ty::I8);
            assert_eq!(i.a, Val::Const(5));
            assert_eq!(i.b, Val::Const(2));
        }
        other => panic!("expected Icmp, got {other:?}"),
    }
}

#[test]
fn parses_sext_with_nneg_attribute() {
    let ll = r#"
define i16 @main() {
  %a = sext nneg i8 0 to i16
  ret i16 %a
}
"#;
    let m = parse_ll(ll);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Sext(s) => {
            assert_eq!(s.dst, "a");
            assert_eq!(s.from, ir::Ty::I8);
            assert_eq!(s.val, Val::Const(0));
            assert_eq!(s.to, ir::Ty::I16);
        }
        other => panic!("expected Sext, got {other:?}"),
    }
}

// Milestone-7 struct surface: type table + layout, struct globals, alloca,
// memcpy, lifetime, paren + multi-index GEPs, inlined-GEP synthesis, byval/
// sret params.
const STRUCTS: &str = r#"
%struct.S = type { i8, i16 }

@g = dso_local global %struct.S zeroinitializer, align 2
@g1 = dso_local global %struct.S zeroinitializer, align 2
@g2 = dso_local global %struct.S zeroinitializer, align 2

define dso_local zeroext i8 @f(ptr nocapture noundef readonly byval(%struct.S) align 2 %0) local_unnamed_addr #0 {
  %2 = load i8, ptr %0, align 2, !tbaa !2
  %3 = getelementptr inbounds nuw i8, ptr %0, i16 2
  %4 = load i16, ptr %3, align 2, !tbaa !7
  ret i8 %2
}

define dso_local void @main() local_unnamed_addr #1 {
  %1 = alloca %struct.S, align 2
  call void @llvm.lifetime.start.p0(i64 4, ptr nonnull %1) #3
  store i8 1, ptr getelementptr inbounds nuw (i8, ptr @g, i16 2), align 2, !tbaa !2
  %2 = getelementptr inbounds nuw i8, ptr %1, i16 2
  %3 = tail call zeroext i8 @f(ptr noundef nonnull byval(%struct.S) align 2 %1) #4
  tail call void @llvm.memcpy.p0.p0.i16(ptr align 2 @g1, ptr align 2 @g2, i16 4, i1 false), !tbaa.struct !2
  call void @llvm.lifetime.end.p0(i64 4, ptr nonnull %1) #3
  ret void
}
"#;

#[test]
fn parses_structs_type_table_globals_alloca_memcpy_gep_and_params() {
    let m = parse_ll(STRUCTS);

    // type table: {i8, i16} -> size 4, the two scalar globals each hold it
    assert_eq!(m.globals.len(), 3);
    for g in &m.globals {
        assert_eq!(g.size, 4, "struct global @{} size", g.name);
        assert_eq!(g.bytes, vec![0u8; 4]);
    }

    let f = &m.funcs[0]; // @f: byval param
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "0");
    assert_eq!(f.params[0].width, 4);
    assert_eq!(f.params[0].byval, Some(4));
    assert!(!f.params[0].sret);

    let body = &f.blocks[0].insts;
    assert_eq!(body.len(), 4); // load, gep %0, load, ret
    match &body[1] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Reg("0".to_string()));
            assert_eq!(g.k, 2);
            assert!(g.terms.is_empty());
        }
        other => panic!("expected Gep, got {other:?}"),
    }

    let main = &m.funcs[1];
    let body = &main.blocks[0].insts;
    // alloca, [inlined-gep @g+2 synth] + store, gep %1+2, call, memcpy, ret = 7
    assert_eq!(body.len(), 7);

    match &body[0] {
        Inst::Alloca(a) => {
            assert_eq!(a.dst, "1");
            assert_eq!(a.size, 4);
        }
        other => panic!("expected Alloca, got {other:?}"),
    }

    // the inlined GEP became a synthetic Gep inst before the store
    match &body[1] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("g".to_string()));
            assert_eq!(g.k, 2);
        }
        other => panic!("expected synthesized Gep, got {other:?}"),
    }
    match &body[2] {
        Inst::Store(s) => {
            assert_eq!(s.ty, ir::Ty::I8);
            assert!(
                s.ptr.starts_with("%__gep"),
                "store ptr must reference the synthesized gep reg: {}",
                s.ptr
            );
        }
        other => panic!("expected Store, got {other:?}"),
    }

    match &body[3] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Reg("1".to_string()));
            assert_eq!(g.k, 2);
        }
        other => panic!("expected Gep, got {other:?}"),
    }
    match &body[4] {
        Inst::Call(c) => {
            assert_eq!(c.func, "f");
            assert_eq!(c.args.len(), 1);
            assert_eq!(c.args[0].ty, None); // ptr arg
            assert_eq!(c.args[0].byval, Some(4));
            assert_eq!(c.args[0].val, Val::Reg("1".to_string()));
        }
        other => panic!("expected Call, got {other:?}"),
    }

    // @g1 = memcpy @g2, 4 bytes (i16 4), non-volatile
    assert!(
        body.iter().any(
            |i| matches!(i, Inst::Memcpy(m) if m.dst == Val::Global("g1".to_string())
            && m.src == Val::Global("g2".to_string()) && m.len == MemLen::Const(4))
        ),
        "memcpy must appear: {body:?}"
    );
    // lifetime.start/end produce no instructions
    assert!(
        !body
            .iter()
            .any(|i| matches!(i, Inst::Call(c) if c.func.starts_with("llvm.lifetime"))),
        "lifetime calls must be skipped"
    );
}

// Milestone 12: the l1.ll i32 shapes — add/icmp/zext/trunc/sext binops and
// casts, volatile load/store, and a call with i32 params/return. clang's
// i32 calls/returns pass through irparse untouched.
#[test]
fn parses_i32_shapes() {
    let ll = r#"
@in = dso_local global i32 0, align 2
@out = dso_local global i32 0, align 2
define dso_local noundef i32 @f(i32 noundef %0, i32 noundef %1) local_unnamed_addr #0 {
  %3 = add i32 %1, %0
  ret i32 %3
}
define dso_local void @main() local_unnamed_addr #1 {
  %1 = load volatile i32, ptr @in, align 2
  %2 = call i32 @f(i32 noundef %1, i32 noundef 5)
  store volatile i32 %2, ptr @out, align 2
  %10 = icmp ult i32 %1, 100
  %11 = zext i1 %10 to i32
  store volatile i32 %11, ptr @out, align 2
  %12 = trunc i32 %1 to i16
  %13 = sext i16 %12 to i32
  store volatile i32 %13, ptr @out, align 2
  ret void
}
"#;
    let m = parse_ll(ll);
    assert_eq!(m.globals.len(), 2);
    assert_eq!(m.globals[0].ty, ir::Ty::I32);
    assert_eq!(m.globals[0].size, 4, "i32 global is 4 bytes");
    assert_eq!(m.globals[1].size, 4);

    // @f: two i32 params (width 4), i32 return.
    let f = &m.funcs[0];
    assert_eq!(f.ret, Some(ir::Ty::I32));
    assert_eq!(f.params.len(), 2);
    assert_eq!(f.params[0].width, 4);
    assert_eq!(f.params[1].width, 4);
    let fbody = &f.blocks[0].insts;
    match &fbody[0] {
        Inst::Bin(b) => {
            assert_eq!(b.op, ir::BinOp::Add);
            assert_eq!(b.ty, ir::Ty::I32);
        }
        other => panic!("expected i32 add, got {other:?}"),
    }
    match &fbody[1] {
        Inst::Ret(Some((ty, _))) => assert_eq!(*ty, ir::Ty::I32),
        other => panic!("expected i32 ret, got {other:?}"),
    }

    let main = &m.funcs[1];
    let body = &main.blocks[0].insts;
    // load i32, call i32, store i32, icmp ult i32, zext i1->i32, store,
    // trunc i32->i16, sext i16->i32, store, ret = 10
    assert_eq!(body.len(), 10);
    assert!(matches!(body.last(), Some(Inst::Ret(None))));
    assert!(matches!(&body[0], Inst::Load(l) if l.ty == ir::Ty::I32));
    match &body[1] {
        Inst::Call(c) => {
            assert_eq!(c.ty, Some(ir::Ty::I32));
            assert_eq!(c.args.len(), 2);
            assert_eq!(c.args[0].ty, Some(ir::Ty::I32));
            assert_eq!(c.args[1].ty, Some(ir::Ty::I32));
        }
        other => panic!("expected i32 call, got {other:?}"),
    }
    assert!(matches!(&body[2], Inst::Store(s) if s.ty == ir::Ty::I32));
    assert!(matches!(&body[3], Inst::Icmp(i) if i.pred == "ult" && i.ty == ir::Ty::I32));
    match &body[4] {
        Inst::Zext(z) => {
            assert_eq!(z.from, ir::Ty::I1);
            assert_eq!(z.to, ir::Ty::I32);
        }
        other => panic!("expected zext i1 to i32, got {other:?}"),
    }
    assert!(matches!(&body[5], Inst::Store(s) if s.ty == ir::Ty::I32));
    match &body[6] {
        Inst::Trunc(t) => {
            assert_eq!(t.from, ir::Ty::I32);
            assert_eq!(t.to, ir::Ty::I16);
        }
        other => panic!("expected trunc i32 to i16, got {other:?}"),
    }
    match &body[7] {
        Inst::Sext(x) => {
            assert_eq!(x.from, ir::Ty::I16);
            assert_eq!(x.to, ir::Ty::I32);
        }
        other => panic!("expected sext i16 to i32, got {other:?}"),
    }
}

// Milestone 12: struct layout — an i32 field is size 4, align 2, so both
// `{ i8, i32 }` and `{ i32, i8 }` come out 6 bytes (i8 @0, i32 @2 in the
// first; i32 @0, i8 @4 in the second, round_up(5, 2) = 6).
#[test]
fn struct_layout_sizes_i32_fields() {
    let ll = r#"
%struct.A = type { i8, i32 }
%struct.B = type { i32, i8 }
@a = dso_local global %struct.A zeroinitializer, align 2
@b = dso_local global %struct.B zeroinitializer, align 2
define dso_local void @main() {
  ret void
}
"#;
    let m = parse_ll(ll);
    assert_eq!(m.globals[0].size, 6, "{{i8, i32}} -> i8@0, i32@2, size 6");
    assert_eq!(m.globals[0].bytes.len(), 6);
    assert_eq!(m.globals[1].size, 6, "{{i32, i8}} -> i32@0, i8@4, size 6");
    assert_eq!(m.globals[1].bytes.len(), 6);
}

// Issue #5: clang -O1 prints const struct globals with EXPANDED literal
// types (explicit padding) — `{ i8, i8, i16 }` for `struct { char; short }`.
// The decode must flatten the initializer into the table's byte blob using
// the same alignment layout as the type table.
const CONST_STRUCTS: &str = r#"
%struct.Pair = type { i8, i16 }
@C1 = dso_local constant { i8, i8, i16 } { i8 65, i8 0, i16 4660 }, align 2
@C2 = dso_local constant { { i8, i8, i16 }, i8, i8 } { { i8, i8, i16 } { i8 66, i8 0, i16 22136 }, i8 67, i8 0 }, align 2
@CA = dso_local constant { [3 x i8], i8, i16 } { [3 x i8] c"abc", i8 0, i16 4951 }, align 2
@CF = dso_local constant { float, i8, i8 } { float 1.500000e+00, i8 81, i8 0 }, align 2
@CARR = dso_local constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2
@CZ = dso_local constant { i8, i8, i16 } zeroinitializer, align 2
@gr = dso_local global { i8, i8, i16 } { i8 71, i8 0, i16 0x0102 }, align 2
define dso_local void @main() {
  ret void
}
"#;

#[test]
fn decodes_literal_struct_initializers_to_flat_bytes() {
    let m = parse_ll(CONST_STRUCTS);
    let g = |n: &str| m.globals.iter().find(|g| g.name == n).unwrap();

    // C1 = { 'A', pad 0, 0x1234 } -> [0x41, 0x00, 0x34, 0x12]
    let c1 = g("C1");
    assert!(c1.is_const);
    assert_eq!(c1.size, 4);
    assert_eq!(c1.bytes, vec![0x41, 0x00, 0x34, 0x12]);

    // C2 = { { 'B', pad, 0x5678 }, 'C', pad } -> size 6
    let c2 = g("C2");
    assert_eq!(c2.size, 6);
    assert_eq!(c2.bytes, vec![0x42, 0x00, 0x78, 0x56, 0x43, 0x00]);

    // CA = { "abc", pad, 0x1357 } -> size 6
    let ca = g("CA");
    assert_eq!(ca.size, 6);
    assert_eq!(ca.bytes, vec![0x61, 0x62, 0x63, 0x00, 0x57, 0x13]);

    // CF = { 1.5f (0x3FC00000 LE), 'Q', pad } -> size 6
    let cf = g("CF");
    assert_eq!(cf.size, 6);
    assert_eq!(cf.bytes, vec![0x00, 0x00, 0xC0, 0x3F, 0x51, 0x00]);

    // CARR = two { i8, i8, i16 } elements -> size 8, concatenated
    let carr = g("CARR");
    assert_eq!(carr.size, 8);
    assert_eq!(
        carr.bytes,
        vec![0x44, 0x00, 0x11, 0x11, 0x45, 0x00, 0x22, 0x22]
    );

    // zeroinitializer literal struct -> zeros of the layout size
    let cz = g("CZ");
    assert_eq!(cz.size, 4);
    assert_eq!(cz.bytes, vec![0u8; 4]);

    // RAM struct with an initializer keeps the same decode (and is not const)
    let gr = g("gr");
    assert!(!gr.is_const);
    assert_eq!(gr.size, 4);
    assert_eq!(gr.bytes, vec![0x47, 0x00, 0x02, 0x01]);
}

// Issue #5 regressions (code review): clang 20.1.8 prints a zero-initialized
// nested struct field as `{ T } zeroinitializer` inside the parent's brace
// list, and array-of-literal-struct element types may themselves contain
// array fields (`[2 x { [2 x { i8, i8, i16 }], i8, i8 }]`). Both shapes must
// decode, not panic.
const CONST_STRUCT_REGRESSIONS: &str = r#"
@W = dso_local constant { { i8, i8, i16 }, i8, i8 } { { i8, i8, i16 } zeroinitializer, i8 120, i8 0 }, align 2
@O2 = dso_local constant [2 x { [2 x { i8, i8, i16 }], i8, i8 }] [{ [2 x { i8, i8, i16 }], i8, i8 } { [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 1, i8 0, i16 2 }, { i8, i8, i16 } { i8 3, i8 0, i16 4 }], i8 97, i8 0 }, { [2 x { i8, i8, i16 }], i8, i8 } { [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 5, i8 0, i16 6 }, { i8, i8, i16 } { i8 7, i8 0, i16 8 }], i8 98, i8 0 }], align 2
define dso_local void @main() {
  ret void
}
"#;

#[test]
fn decodes_nested_zeroinit_and_array_field_structs() {
    let m = parse_ll(CONST_STRUCT_REGRESSIONS);
    let g = |n: &str| m.globals.iter().find(|g| g.name == n).unwrap();

    // W = { { 0, 0 }, 'x', pad } — nested Pair zeroinitializer, size 6
    let w = g("W");
    assert_eq!(w.size, 6);
    assert_eq!(w.bytes, vec![0x00, 0x00, 0x00, 0x00, 0x78, 0x00]);

    // O2 = two elements; each { Pair[2], 'a'/'b', pad } — size 10 each.
    // Element 0: Pairs (1,2),(3,4) then 'a'; element 1: (5,6),(7,8) then 'b'.
    let o2 = g("O2");
    assert_eq!(o2.size, 20);
    let mut expect = Vec::new();
    for e in [&[1u8, 0, 2, 0, 3, 0, 4, 0][..], &[5, 0, 6, 0, 7, 0, 8, 0]] {
        expect.extend_from_slice(e);
        expect.push(if e[0] == 1 { b'a' } else { b'b' });
        expect.push(0);
    }
    assert_eq!(o2.bytes, expect);
}

// Issue #5: clang -O1 lowers `&CARR[i]` on a const struct array to
// `getelementptr [2 x %struct.Pair], ptr @CARR, i16 0, i16 %i` — the index
// after an array-of-struct descent is the ELEMENT selector, striding by
// sizeof(%struct.Pair). Field offsets ride as separate i8-offset GEPs, so
// no further struct descent is needed.
const STRUCT_ARRAY_GEP: &str = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2
define dso_local void @main(i16 %i) {
  %p = getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16 %i
  ret void
}
"#;

#[test]
fn folds_struct_array_element_gep_to_struct_stride() {
    let m = parse_ll(STRUCT_ARRAY_GEP);
    let body = &m.funcs[0].blocks[0].insts;
    match &body[0] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("CARR".to_string()));
            assert_eq!(g.k, 0);
            assert_eq!(
                g.terms,
                vec![(4, "i".to_string())],
                "element stride = sizeof(%struct.Pair) = 4"
            );
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

#[test]
fn folds_struct_array_constant_element_gep_to_byte_offset() {
    let ll = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2
define dso_local void @main() {
  %p = getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16 1
  ret void
}
"#;
    let m = parse_ll(ll);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Gep(g) => {
            assert_eq!(g.k, 4, "constant element 1 -> byte offset 4");
            assert!(g.terms.is_empty());
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

// s8: chained multi-index GEP with an inlined base GEP (dynamic struct-array).
const CHAINED: &str = r#"
%struct.A = type { i8, [4 x i8] }
@a = dso_local global %struct.A zeroinitializer, align 1
define dso_local void @main() local_unnamed_addr #0 {
  %1 = load volatile i8, ptr @a, align 1, !tbaa !2
  %2 = zext i8 %1 to i16
  %3 = getelementptr inbounds nuw [4 x i8], ptr getelementptr inbounds nuw (i8, ptr @a, i16 1), i16 0, i16 %2
  store volatile i8 7, ptr %3, align 1, !tbaa !6
  ret void
}
"#;

#[test]
fn parses_chained_multi_index_gep_with_inlined_base() {
    let m = parse_ll(CHAINED);
    assert_eq!(m.globals[0].size, 5, "{{i8,[4xi8]}} -> 5");

    let body = &m.funcs[0].blocks[0].insts;
    // load, zext, [synth gep @a+1], gep %__gep +0 +1*%2, store, ret = 6
    assert_eq!(body.len(), 6);

    // inner inlined base GEP materialized first: gep @a +1
    let synth_dst;
    match &body[2] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("a".to_string()));
            assert_eq!(g.k, 1);
            assert!(g.terms.is_empty());
            assert!(g.dst.starts_with("__gep"), "synthetic reg: {}", g.dst);
            synth_dst = g.dst.clone();
        }
        other => panic!("expected synthesized base Gep, got {other:?}"),
    }

    // outer GEP: base = the synthetic reg, k 0, dynamic term 1*%2
    match &body[3] {
        Inst::Gep(g) => match &g.base {
            GepBase::Reg(r) => {
                assert_eq!(r, &synth_dst);
                assert_eq!(g.k, 0);
                assert_eq!(g.terms, vec![(1, "2".to_string())]);
            }
            other => panic!("expected Reg base, got {other:?}"),
        },
        other => panic!("expected Gep, got {other:?}"),
    }
}

// s7: sret param.
const SRET: &str = r#"
%struct.S = type { i8, i16 }
define dso_local void @make(ptr dead_on_unwind noalias nocapture writable writeonly sret(%struct.S) align 2 initializes((0, 1), (2, 4)) %0) local_unnamed_addr #0 {
  store i8 1, ptr %0, align 2, !tbaa !2
  ret void
}
define dso_local void @main() local_unnamed_addr #1 {
  %1 = alloca %struct.S, align 2
  call void @make(ptr dead_on_unwind nonnull writable sret(%struct.S) align 2 %1) #5
  ret void
}
"#;

#[test]
fn parses_sret_param_and_call_arg() {
    let m = parse_ll(SRET);

    let make = &m.funcs[0];
    assert_eq!(make.params.len(), 1);
    assert_eq!(make.params[0].name, "0");
    assert_eq!(make.params[0].width, 2); // sret slot holds an address
    assert!(make.params[0].sret);
    assert_eq!(make.params[0].byval, None);

    let main_body = &m.funcs[1].blocks[0].insts;
    match &main_body[1] {
        Inst::Call(c) => {
            assert_eq!(c.func, "make");
            assert_eq!(c.args.len(), 1);
            assert!(c.args[0].sret);
            assert_eq!(c.args[0].ty, None);
            assert_eq!(c.args[0].val, Val::Reg("1".to_string()));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

// Fix (1): unknown param type tokens must panic loudly, not silently
// mis-parse into a width-2 slot. i32 is now supported (M12), so use an
// unsupported type token (i24) to exercise the loud panic.
#[test]
#[should_panic(expected = "i24")]
fn param_unknown_type_token_panics() {
    let ll = r#"
define dso_local void @f(i24 %x) {
  ret void
}
"#;
    let _ = parse_ll(ll);
}

// Issue #5: by-value struct-element args carry BOTH the byval attr and an
// inlined GEP (`ptr ... byval(%struct.S) align 2 getelementptr ...`). The
// inlined-GEP branch must preserve the attr or isel's byval copy is
// skipped and the callee ABI silently breaks.
const BYVAL_GEP_ARG: &str = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2
define dso_local void @take_byval(ptr nocapture noundef readonly byval(%struct.Pair) align 2 %0) local_unnamed_addr #0 {
  ret void
}
define dso_local void @main() local_unnamed_addr #1 {
  tail call void @take_byval(ptr noundef nonnull byval(%struct.Pair) align 2 getelementptr inbounds nuw (i8, ptr @CARR, i16 4))
  ret void
}
"#;

#[test]
fn preserves_byval_on_inlined_gep_call_arg() {
    let m = parse_ll(BYVAL_GEP_ARG);
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    // The inlined GEP is synthesized BEFORE the Call (insts[0] = gep).
    assert!(
        matches!(main.blocks[0].insts[0], Inst::Gep(_)),
        "synth GEP first"
    );
    match &main.blocks[0].insts[1] {
        Inst::Call(c) => {
            assert_eq!(c.func, "take_byval");
            assert_eq!(c.args.len(), 1);
            let arg = &c.args[0];
            assert_eq!(arg.byval, Some(4), "byval(%struct.Pair) -> size 4");
            assert!(!arg.sret);
            assert!(
                matches!(arg.val, Val::Reg(_)),
                "inlined GEP is synthesized into a reg: {:?}",
                arg.val
            );
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

// Fix (4): nonzero const GEP prefix folds into k (regression guard for the
// latent M5 fix): `getelementptr [4 x i8], ptr @x, i16 1, i16 %2` -> k=4 +
// term (%2, 1).
#[test]
fn folds_nonzero_const_gep_prefix() {
    let ll = r#"
@x = dso_local global [4 x i8] zeroinitializer, align 1
define dso_local void @main() {
  %p = getelementptr [4 x i8], ptr @x, i16 1, i16 %2
  ret void
}
"#;
    let m = parse_ll(ll);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("x".to_string()));
            assert_eq!(g.k, 4);
            assert_eq!(g.terms, vec![(1, "2".to_string())]);
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

// Fix (5): volatile memcpy (`i1 true`) is accepted as an identical byte copy
// (isvolatile is an LLVM optimization hint). s2.ll shape.
const S2_VOLATILE_MEMCPY: &str = r#"
%struct.S = type { i8, i16 }
@g1 = dso_local global %struct.S zeroinitializer, align 2
@g2 = dso_local global %struct.S zeroinitializer, align 2
define dso_local void @main() local_unnamed_addr #0 {
  tail call void @llvm.memcpy.p0.p0.i16(ptr align 2 @g1, ptr align 2 @g2, i16 4, i1 true), !tbaa.struct !2
  ret void
}
"#;

#[test]
fn parses_volatile_memcpy() {
    let m = parse_ll(S2_VOLATILE_MEMCPY);
    let body = &m.funcs[0].blocks[0].insts;
    assert!(
        body.iter().any(|i| matches!(i, Inst::Memcpy(mm)
            if mm.len == MemLen::Const(4) && mm.dst == Val::Global("g1".to_string())
                && mm.src == Val::Global("g2".to_string()))),
        "volatile memcpy must parse to Memcpy{{len:Const(4)}}: {body:?}"
    );
}

// Issue #4: a runtime-length memcpy (`i16 %n`) parses to `MemLen::Reg` — the
// counted-loop form — instead of panicking on the non-const length.
#[test]
fn parses_runtime_length_memcpy() {
    let src = r#"
@b1 = dso_local global [16 x i8] zeroinitializer, align 1
@b2 = dso_local global [16 x i8] zeroinitializer, align 1
define dso_local void @main(i16 %n) {
  tail call void @llvm.memcpy.p0.p0.i16(ptr align 1 @b1, ptr align 1 @b2, i16 %n, i1 false)
  ret void
}
"#;
    let m = parse_ll(src);
    let body = &m.funcs[0].blocks[0].insts;
    assert!(
        body.iter().any(|i| matches!(i, Inst::Memcpy(mm)
            if mm.len == MemLen::Reg(Val::Reg("n".to_string()))
                && mm.dst == Val::Global("b1".to_string())
                && mm.src == Val::Global("b2".to_string()))),
        "runtime-len memcpy must parse to MemLen::Reg(%n): {body:?}"
    );
}

// Fix (2): struct-typed GEP sources now decode field selectors to byte
// offsets (field 1 of `{i8,i16}` is at offset 2). The previous panic for
// any struct GEP is now only for register field indices.
#[test]
fn struct_gep_source_with_field_selector_decodes() {
    let ll = r#"
%struct.S = type { i8, i16 }
@s = dso_local global %struct.S zeroinitializer, align 2
define dso_local void @main() {
  %p = getelementptr %struct.S, ptr @s, i16 0, i16 1
  ret void
}
"#;
    let m = parse_ll(ll);
    let gep = m.funcs[0].blocks[0]
        .insts
        .iter()
        .find_map(|i| match i {
            Inst::Gep(g) => Some(g),
            _ => None,
        })
        .expect("gep must exist");
    assert_eq!(gep.base, GepBase::Global("s".to_string()));
    assert_eq!(gep.k, 2, "field 1 of {{i8,i16}} at offset 2");
    assert!(gep.terms.is_empty());
}

#[test]
#[should_panic(expected = "field index cannot be a register")]
fn struct_gep_register_field_index_panics() {
    let ll = r#"
%struct.S = type { i8, i16 }
@s = dso_local global %struct.S zeroinitializer, align 2
define dso_local void @main() {
  %r = load i16, ptr @s, align 2
  %p = getelementptr %struct.S, ptr @s, i16 0, i16 %r
  ret void
}
"#;
    let _ = parse_ll(ll);
}

// Fix (3): struct sizes/offsets exceeding 255 must assert.
#[test]
#[should_panic(expected = "255")]
fn oversized_struct_panics() {
    let ll = r#"
%struct.Big = type { [300 x i8] }
define dso_local void @main() {
  ret void
}
"#;
    let _ = parse_ll(ll);
}

// Milestone 8: mul/udiv/shl/lshr (m3) and sdiv/srem/ashr (m4) probe shapes,
// plus `freeze`. `volatile` is stripped like any other attr.
const M3_M4_BINOPS: &str = r#"
@in = dso_local global i16 0, align 2
@out = dso_local global i16 0, align 2
define dso_local void @main() {
  %1 = load volatile i16, ptr @in, align 2
  %2 = freeze i16 %1
  %3 = udiv i16 %2, 7
  %4 = mul i16 %3, 7
  %7 = shl i16 %4, 3
  %9 = lshr i16 %7, 1
  %10 = sdiv i16 %9, -3
  %11 = srem i16 %10, 3
  %12 = ashr i16 %11, 2
  store volatile i16 %12, ptr @out, align 2
  ret void
}
"#;

#[test]
fn parses_m3_m4_binops_and_freeze() {
    let m = parse_ll(M3_M4_BINOPS);
    let body = &m.funcs[0].blocks[0].insts;

    // freeze: canonical `%d = freeze <ty> <val>`
    match &body[1] {
        Inst::Freeze(f) => {
            assert_eq!(f.dst, "2");
            assert_eq!(f.ty, ir::Ty::I16);
            assert_eq!(f.val, Val::Reg("1".to_string()));
        }
        other => panic!("expected Freeze, got {other:?}"),
    }

    // each new binop opcode maps to the matching BinOp
    let binned: Vec<(String, ir::BinOp)> = body
        .iter()
        .filter_map(|i| match i {
            Inst::Bin(b) => Some((b.dst.clone(), b.op)),
            _ => None,
        })
        .collect();
    let expected = [
        ("3", ir::BinOp::UDiv),
        ("4", ir::BinOp::Mul),
        ("7", ir::BinOp::Shl),
        ("9", ir::BinOp::LShr),
        ("10", ir::BinOp::SDiv),
        ("11", ir::BinOp::SRem),
        ("12", ir::BinOp::AShr),
    ];
    for (dst, op) in expected {
        assert!(
            binned.contains(&(dst.to_string(), op)),
            "missing {dst} {op:?} in {body:?}"
        );
    }
}

// Probe from /tmp/m7probe/i2.ll (trimmed): the interrupt marker
// (`msp430_intrcc` in the return position) and SFR access via
// `inttoptr (<ty> <k> to ptr)` constant pointers.
const ISR_INTTOPTR: &str = r#"
define dso_local msp430_intrcc void @isr() #0 {
  store volatile i8 85, ptr inttoptr (i16 6 to ptr), align 2, !tbaa !2
  ret void
}

define dso_local void @main() local_unnamed_addr #1 {
  store volatile i8 17, ptr inttoptr (i16 6 to ptr), align 2, !tbaa !2
  ret void
}
"#;

#[test]
fn parses_isr_marker_and_inttoptr() {
    let m = parse_ll(ISR_INTTOPTR);
    assert_eq!(m.funcs.len(), 2);
    // the msp430_intrcc return token -> Func.isr == true, ret stays void
    let isr = m.funcs.iter().find(|f| f.name == "isr").unwrap();
    assert!(isr.isr, "msp430_intrcc must set Func.isr == true");
    assert_eq!(isr.ret, None, "the ISR's ret type stays void");
    // main is not an ISR
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    assert!(!main.isr);
    assert_eq!(main.ret, None);
    // the store's ptr is the literal inttoptr form
    match &main.blocks[0].insts[0] {
        Inst::Store(s) => assert_eq!(
            s.ptr, "0x06",
            "inttoptr (i16 6 to ptr) -> literal ptr '0x06'"
        ),
        other => panic!("expected Store, got {other:?}"),
    }
    match &isr.blocks[0].insts[0] {
        Inst::Store(s) => assert_eq!(s.ptr, "0x06"),
        other => panic!("expected Store, got {other:?}"),
    }
    // the canonical text round-trips: serialize -> parse -> serialize stable
    let out = ir::serialize(&m);
    assert!(
        out.contains("fn isr(void) [isr] ()"),
        "isr marker header\n---\n{out}"
    );
    assert!(
        out.contains("store i8 85 0x06"),
        "literal ptr store\n---\n{out}"
    );
    let m2 = ir::parse(&out);
    assert_eq!(ir::serialize(&m2), out, "stable fixed point");
    let m2isr = m2.funcs.iter().find(|f| f.name == "isr").unwrap();
    assert!(m2isr.isr, "isr marker must round-trip");
}

#[test]
fn parses_inttoptr_load() {
    let src = "define dso_local void @main() {\n  %1 = load volatile i8, ptr inttoptr (i16 6 to ptr), align 2\n  ret void\n}\n";
    let m = parse_ll(src);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Load(l) => assert_eq!(l.ptr, "0x06", "inttoptr load ptr -> literal '0x06'"),
        other => panic!("expected Load, got {other:?}"),
    }
}

// Milestone 10 + issue #8: 16-bit const table sizes. A `[N x i8] constant`
// table with 256 <= N <= 65535 parses (bytes = the literal, size = N) — the
// reader generalizes to as many 256-byte chunks as needed (the 511-byte
// two-chunk bound was the issue-#8 scope limit); a RAM `[N x i8] global`
// array keeps N <= 255 (loud). The device flash bound is enforced later by
// the assembler (code + tables must fit `device.flash_words`).

/// Build a `c"..."` literal hex-escaped for the byte pattern
/// `i -> (i * 37 + 11) & 0xFF` (length `n`), so every byte is distinct-ish
/// and the escaped literal is exactly `n` bytes.
fn escaped_literal(n: usize) -> String {
    (0..n)
        .map(|i| format!("\\{:02X}", ((i as u8).wrapping_mul(37).wrapping_add(11))))
        .collect()
}

#[test]
fn const_array_300_bytes_parses() {
    let lit = escaped_literal(300);
    let src = format!("@table = dso_local constant [300 x i8] c\"{lit}\", align 1\n");
    let m = parse_ll(&src);
    assert_eq!(m.globals.len(), 1);
    match &m.globals[0] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "table");
            assert_eq!(*ty, ir::Ty::I8);
            assert!(*is_const, "const table must be flagged const");
            assert_eq!(*size, 300, "const table size must be the byte count");
            assert_eq!(bytes.len(), 300, "bytes must be the full literal");
            assert_eq!(*addr, None);
        }
    }
    // spot-check: bytes decode back through the pattern
    assert_eq!(m.globals[0].bytes[0], 0x0B); // (0*37 + 11) & 0xFF
    assert_eq!(m.globals[0].bytes[299], 66); // (299*37 + 11) & 0xFF
}

// Issue #3: const tables of multi-byte elements. clang prints an
// `[N x i32]`/`[N x float]` constant as a typed element list
// (`[i32 286331153, i32 572662306, ...]` / `[float 0x3FB99999A0000000, ...]`)
// — never a `c"..."` string — so the const-in-flash path must decode the
// elements into the table's little-endian byte blob (byte i of element e is
// table byte e*elem_size + i). A float constant clang cannot print in 8 hex
// digits (e.g. 0.1f) appears as its f64-promoted bit pattern, which must be
// narrowed back to the f32 bits exactly like the operand parser does.
#[test]
fn const_array_i32_elements_parse_to_le_bytes() {
    let src = r#"
@itable = dso_local constant [3 x i32] [i32 0x11111111, i32 0x22222222, i32 -2], align 2
"#;
    let m = parse_ll(src);
    assert_eq!(m.globals.len(), 1);
    match &m.globals[0] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "itable");
            assert_eq!(*ty, ir::Ty::I32);
            assert!(*is_const);
            assert_eq!(*size, 12, "3 x i32 = 12 bytes");
            assert_eq!(
                bytes,
                &vec![
                    0x11, 0x11, 0x11, 0x11, // 0x11111111 LE
                    0x22, 0x22, 0x22, 0x22, // 0x22222222 LE
                    0xFE, 0xFF, 0xFF, 0xFF, // -2 LE
                ]
            );
            assert_eq!(*addr, None);
        }
    }
}

#[test]
fn const_array_float_elements_parse_to_le_bytes() {
    // 0x3F800000 = 1.0f; 0x3FB99999A0000000 is the f64 promotion of 0.1f
    // (clang prints it for float constants not representable in 8 hex
    // digits) and must narrow to 0x3DCCCCCD; 5.000000e-01 is the decimal
    // form of 0.5f -> 0x3F000000.
    let src = r#"
@ftable = dso_local constant [3 x float] [float 1.000000e+00, float 0x3FB99999A0000000, float 5.000000e-01], align 2
"#;
    let m = parse_ll(src);
    assert_eq!(m.globals.len(), 1);
    match &m.globals[0] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "ftable");
            assert_eq!(*ty, ir::Ty::F32);
            assert!(*is_const);
            assert_eq!(*size, 12, "3 x float = 12 bytes");
            assert_eq!(
                bytes,
                &vec![
                    0x00, 0x00, 0x80, 0x3F, // 1.0f
                    0xCD, 0xCC, 0xCC, 0x3D, // 0.1f
                    0x00, 0x00, 0x00, 0x3F, // 0.5f
                ]
            );
            assert_eq!(*addr, None);
        }
    }
}

#[test]
fn const_array_600_bytes_parses() {
    // Issue #8: past the old 511-byte two-chunk bound — 600 bytes needs
    // three 256-byte chunks (chunks 0, 1, 2 covering 0..255, 256..511,
    // 512..599).
    let lit = escaped_literal(600);
    let src = format!("@big = dso_local constant [600 x i8] c\"{lit}\", align 1\n");
    let m = parse_ll(&src);
    assert_eq!(m.globals.len(), 1);
    match &m.globals[0] {
        Global {
            name,
            ty,
            is_const,
            size,
            bytes,
            addr,
        } => {
            assert_eq!(name, "big");
            assert_eq!(*ty, ir::Ty::I8);
            assert!(*is_const);
            assert_eq!(*size, 600, "const table size must be the byte count");
            assert_eq!(bytes.len(), 600, "bytes must be the full literal");
            assert_eq!(*addr, None);
        }
    }
    assert_eq!(
        m.globals[0].bytes[511],
        (511u32.wrapping_mul(37).wrapping_add(11)) as u8,
        "chunk-boundary byte"
    );
    assert_eq!(
        m.globals[0].bytes[512],
        (512u32.wrapping_mul(37).wrapping_add(11)) as u8,
        "first byte past the old bound"
    );
    assert_eq!(
        m.globals[0].bytes[599],
        (599u32.wrapping_mul(37).wrapping_add(11)) as u8,
        "last byte"
    );
}

#[test]
fn const_array_65535_bytes_parses() {
    // The 16-bit index space's ceiling: 256 chunks exactly.
    let lit = escaped_literal(65535);
    let src = format!("@big = dso_local constant [65535 x i8] c\"{lit}\", align 1\n");
    let m = parse_ll(&src);
    assert_eq!(m.globals.len(), 1);
    assert_eq!(m.globals[0].size, 65535);
    assert_eq!(m.globals[0].bytes.len(), 65535);
}

#[test]
#[should_panic(expected = "array @ram too large")]
fn ram_array_300_bytes_panics() {
    let src = "@ram = dso_local global [300 x i8] zeroinitializer, align 1\n";
    let _ = parse_ll(&src);
}

#[test]
fn skips_llvm_bookkeeping_globals() {
    // clang emits @llvm.used / @llvm.compiler.used metadata globals for
    // address-taken symbols — e.g. the interrupt handler:
    // `@llvm.compiler.used = appending global [1 x ptr] [ptr @isr]`. They
    // are backend bookkeeping, not data the PIC8 backend consumes, so they
    // must be skipped (like the llvm.lifetime calls) instead of panicking
    // on the unsupported `ptr` element type.
    let src = r#"
@llvm.compiler.used = appending global [1 x ptr] [ptr @isr] section "llvm.metadata"
@llvm.used = appending global [1 x ptr] [ptr @main] section "llvm.metadata"

define dso_local msp430_intrcc void @isr() #0 {
  ret void
}

define dso_local void @main() #1 {
  ret void
}
"#;
    let m = parse_ll(src);
    assert_eq!(m.globals.len(), 0, "llvm.* globals must be skipped");
    assert_eq!(m.funcs.len(), 2);
    assert!(
        m.funcs.iter().any(|f| f.isr && f.name == "isr"),
        "the isr function must still parse"
    );
    assert!(m.funcs.iter().any(|f| !f.isr && f.name == "main"));
}

#[test]
fn parses_poison_call_arg_as_zero() {
    // clang -O1 emits `poison` for a dead call arg (a value that is never
    // observed: the optimizer specialized a noinline helper and left the
    // original call with a poison arg). A poison operand is consumed only
    // by UB, so a conforming program never reads it — materializing it as
    // 0 is sound, and the parser must accept it (found by the fuzz corpus).
    let src = r#"
define dso_local void @main() {
  %1 = call i8 @f(i8 poison)
  ret void
}

define dso_local i8 @f(i8 %x) {
  ret i8 0
}
"#;
    let m = parse_ll(src);
    assert_eq!(m.funcs.len(), 2);
    let call = &m.funcs[0].blocks[0].insts[0];
    match call {
        Inst::Call(c) => {
            assert_eq!(c.func, "f");
            assert_eq!(c.dst.as_deref(), Some("1"));
            assert_eq!(c.args.len(), 1);
            assert_eq!(
                c.args[0],
                CallArg {
                    ty: Some(ir::Ty::I8),
                    val: Val::Const(0),
                    byval: None,
                    sret: false
                },
                "a poison arg is never observed, so Const(0) is the correct materialization"
            );
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

// Milestone 15: the f1.ll float shapes — fadd, fcmp olt, fptosi, sitofp,
// a float select, and a float-typed global/param/ret.
const F1: &str = r#"
@in = dso_local global float 0.000000e+00, align 2
@out = dso_local global float 0.000000e+00, align 2
@outi = dso_local global i16 0, align 2

define dso_local noundef float @fadd(float noundef %0, float noundef %1) {
  %3 = fadd float %0, %1
  ret float %3
}

define dso_local void @main() {
  %1 = load volatile float, ptr @in, align 2
  %6 = fcmp olt float %1, 1.000000e+00
  %7 = select i1 %6, float 1.000000e+00, float 0.000000e+00
  store volatile float %7, ptr @out, align 2
  %8 = fptosi float %1 to i16
  store volatile i16 %8, ptr @outi, align 2
  %9 = load volatile i16, ptr @outi, align 2
  %10 = sitofp i16 %9 to float
  store volatile float %10, ptr @out, align 2
  ret void
}
"#;

#[test]
fn parses_f1_float_shapes() {
    let m = parse_ll(F1);
    // float globals are 4 bytes; @in/@out are float, @outi is i16.
    assert_eq!(m.globals[0].ty, ir::Ty::F32);
    assert_eq!(m.globals[0].size, 4);
    assert_eq!(m.globals[2].ty, ir::Ty::I16);
    // @fadd: float ret + two float (width-4) params.
    let fadd = m.funcs.iter().find(|f| f.name == "fadd").unwrap();
    assert_eq!(fadd.ret, Some(ir::Ty::F32));
    assert_eq!(fadd.params[0].width, 4);
    assert_eq!(fadd.params[1].width, 4);
    match &fadd.blocks[0].insts[0] {
        Inst::FloatBin(b) => {
            assert_eq!(b.dst, "3");
            assert_eq!(b.op, ir::FBinOp::FAdd);
            assert_eq!(b.a, Val::Reg("0".to_string()));
            assert_eq!(b.b, Val::Reg("1".to_string()));
        }
        other => panic!("expected FloatBin, got {other:?}"),
    }
    // @main: the f1.ll shapes in order.
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    let i = &main.blocks[0].insts;
    match &i[0] {
        Inst::Load(l) => assert_eq!(l.ty, ir::Ty::F32),
        other => panic!("expected Load, got {other:?}"),
    }
    // fcmp olt float %1, 1.0 -> b materialized as the f32 bit pattern.
    match &i[1] {
        Inst::Fcmp(c) => {
            assert_eq!(c.dst, "6");
            assert_eq!(c.pred, "olt");
            assert_eq!(c.a, Val::Reg("1".to_string()));
            assert_eq!(c.b, Val::Const(1.0f32.to_bits() as i64));
        }
        other => panic!("expected Fcmp, got {other:?}"),
    }
    // select i1 %6, float 1.0, float 0.0 -> F32-typed select with bit patterns.
    match &i[2] {
        Inst::Select(s) => {
            assert_eq!(s.ty, ir::Ty::F32);
            assert_eq!(s.a, Val::Const(1.0f32.to_bits() as i64));
            assert_eq!(s.b, Val::Const(0.0f32.to_bits() as i64));
        }
        other => panic!("expected Select, got {other:?}"),
    }
    // fptosi float %1 to i16.
    match &i[4] {
        Inst::FloatConv(c) => {
            assert_eq!(c.op, ir::FloatConvOp::FpToSi);
            assert_eq!(c.from, ir::Ty::F32);
            assert_eq!(c.val, Val::Reg("1".to_string()));
            assert_eq!(c.to, ir::Ty::I16);
        }
        other => panic!("expected FloatConv, got {other:?}"),
    }
    // sitofp i16 %9 to float.
    match &i[7] {
        Inst::FloatConv(c) => {
            assert_eq!(c.op, ir::FloatConvOp::SiToFp);
            assert_eq!(c.from, ir::Ty::I16);
            assert_eq!(c.val, Val::Reg("9".to_string()));
            assert_eq!(c.to, ir::Ty::F32);
        }
        other => panic!("expected FloatConv, got {other:?}"),
    }
}

#[test]
fn parses_f32_constant_forms() {
    // Both .ll f32 constant forms materialize as the 32-bit bit pattern in
    // `Val::Const`: the hex form `f32 0x3F800000` and the decimal forms
    // `float 1.000000e+00` / `float 5.000000e-01` (via f32::from_str +
    // to_bits).
    const C: &str = r#"
@c = global f32 0x3F800000
@d = global float 1.000000e+00
define float @g() {
  %1 = fadd float 0x3F800000, 1.000000e+00
  %2 = fadd float %1, 5.000000e-01
  %3 = fadd float 0x3FB99999A0000000, 0x3FF8000000000000
  %4 = fadd float %3, 0x3FF0000000000000
  ret float %4
}
"#;
    let m = parse_ll(C);
    assert_eq!(m.globals[0].ty, ir::Ty::F32);
    assert_eq!(m.globals[0].size, 4);
    assert_eq!(m.globals[1].ty, ir::Ty::F32);
    let g = &m.funcs[0];
    // hex operand 0x3F800000 == decimal operand 1.0f == same bit pattern.
    match &g.blocks[0].insts[0] {
        Inst::FloatBin(b) => {
            assert_eq!(b.a, Val::Const(0x3F800000u32 as i64), "hex f32 form");
            assert_eq!(b.b, Val::Const(1.0f32.to_bits() as i64), "decimal 1.0f");
        }
        other => panic!("expected FloatBin, got {other:?}"),
    }
    // decimal 0.5f -> 0x3F000000.
    match &g.blocks[0].insts[1] {
        Inst::FloatBin(b) => assert_eq!(b.b, Val::Const(0.5f32.to_bits() as i64)),
        other => panic!("expected FloatBin, got {other:?}"),
    }
    // clang prints f32 constants that do not fit 8 hex digits as their
    // DOUBLE-precision promotion (the M15 float differential's seed-2 bug:
    // `store volatile float 0x3FB99999A0000000` stored the low 32 bits
    // 0xA0000000 instead of 0.1f's 0x3DCCCCCD). The >8-digit hex on an f32
    // operand must round the f64 VALUE back to the f32 bit pattern:
    // 0x3FB99999A0000000 = 0.1f promoted, 0x3FF8000000000000 = 1.5f
    // promoted, 0x3FF0000000000000 = 1.0f (its promotion is exact).
    match &g.blocks[0].insts[2] {
        Inst::FloatBin(b) => {
            assert_eq!(
                b.a,
                Val::Const(0.1f32.to_bits() as i64),
                "0.1f promoted hex"
            );
            assert_eq!(
                b.b,
                Val::Const(1.5f32.to_bits() as i64),
                "1.5f promoted hex"
            );
        }
        other => panic!("expected FloatBin, got {other:?}"),
    }
    match &g.blocks[0].insts[3] {
        Inst::FloatBin(b) => assert_eq!(
            b.b,
            Val::Const(1.0f32.to_bits() as i64),
            "1.0f promoted hex"
        ),
        other => panic!("expected FloatBin, got {other:?}"),
    }
}

#[test]
fn parses_f64_promoted_constants_at_untyped_sites() {
    // clang prints an f32 constant that does not fit 8 hex digits as its
    // DOUBLE-precision promotion in EVERY operand position, not just the
    // typed float-binop/store/freeze/fcmp/conversion operands. The untyped
    // sites (ret/select/phi/call args/icmp/zext/sext/trunc) must apply the
    // same f64->f32 round-trip: `ret float 0x3FB99999A0000000` (0.1f
    // promoted) must materialize 0x3DCCCCCD, not the low-32 truncation
    // 0xA0000000 (the M15 float differential's seed-2 bug shape).
    const S: &str = r#"
define float @ret_promoted() {
  ret float 0x3FB99999A0000000
}
define float @select_promoted(i1 %c) {
  %1 = select i1 %c, float 0x3FB99999A0000000, float 0x3FF8000000000000
  ret float %1
}
define float @phi_promoted(i1 %c) {
entry:
  br i1 %c, label %t, label %f
t:
  br label %m
f:
  br label %m
m:
  %1 = phi float [ 0x3FB99999A0000000, %t ], [ 0x3FF8000000000000, %f ]
  ret float %1
}
define i1 @icmp_promoted() {
  %1 = icmp eq float 0x3FB99999A0000000, 0x3FF8000000000000
  ret i1 %1
}
define i32 @zext_promoted() {
  %1 = zext float 0x3FB99999A0000000 to i32
  ret i32 %1
}
define void @call_promoted() {
  %1 = tail call float @ret_promoted(float noundef 0x3FB99999A0000000)
  ret void
}
"#;
    let m = parse_ll(S);
    let f = |name: &str| m.funcs.iter().find(|f| f.name == name).unwrap();
    // ret float 0x3FB99999A0000000 -> Const(0.1f bits), not the low-32
    // truncation 0xA0000000.
    match &f("ret_promoted").blocks[0].insts[0] {
        Inst::Ret(Some((ty, val))) => {
            assert_eq!(*ty, ir::Ty::F32);
            assert_eq!(
                *val,
                Val::Const(0.1f32.to_bits() as i64),
                "ret promoted hex"
            );
        }
        other => panic!("expected Ret, got {other:?}"),
    }
    // select with a float constant: both arms decode the f64 promotion.
    match &f("select_promoted").blocks[0].insts[0] {
        Inst::Select(s) => {
            assert_eq!(s.ty, ir::Ty::F32);
            assert_eq!(
                s.a,
                Val::Const(0.1f32.to_bits() as i64),
                "select a promoted hex"
            );
            assert_eq!(
                s.b,
                Val::Const(1.5f32.to_bits() as i64),
                "select b promoted hex"
            );
        }
        other => panic!("expected Select, got {other:?}"),
    }
    // phi incoming values decode the f64 promotion.
    let phi_blk = f("phi_promoted")
        .blocks
        .iter()
        .find(|b| b.label == "m")
        .unwrap();
    match &phi_blk.insts[0] {
        Inst::Phi(p) => {
            assert_eq!(p.ty, ir::Ty::F32);
            assert_eq!(
                p.incoming[0].0,
                Val::Const(0.1f32.to_bits() as i64),
                "phi promoted hex"
            );
            assert_eq!(
                p.incoming[1].0,
                Val::Const(1.5f32.to_bits() as i64),
                "phi promoted hex"
            );
        }
        other => panic!("expected Phi, got {other:?}"),
    }
    // icmp float operands decode the f64 promotion.
    match &f("icmp_promoted").blocks[0].insts[0] {
        Inst::Icmp(c) => {
            assert_eq!(c.ty, ir::Ty::F32);
            assert_eq!(
                c.a,
                Val::Const(0.1f32.to_bits() as i64),
                "icmp a promoted hex"
            );
            assert_eq!(
                c.b,
                Val::Const(1.5f32.to_bits() as i64),
                "icmp b promoted hex"
            );
        }
        other => panic!("expected Icmp, got {other:?}"),
    }
    // zext float operand decodes the f64 promotion.
    match &f("zext_promoted").blocks[0].insts[0] {
        Inst::Zext(z) => {
            assert_eq!(z.from, ir::Ty::F32);
            assert_eq!(
                z.val,
                Val::Const(0.1f32.to_bits() as i64),
                "zext promoted hex"
            );
        }
        other => panic!("expected Zext, got {other:?}"),
    }
    // call float arg decodes the f64 promotion.
    match &f("call_promoted").blocks[0].insts[0] {
        Inst::Call(c) => {
            assert_eq!(c.args[0].ty, Some(ir::Ty::F32));
            assert_eq!(
                c.args[0].val,
                Val::Const(0.1f32.to_bits() as i64),
                "call arg promoted hex"
            );
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

#[test]
fn float_struct_layout() {
    // f32 is size 4, align 2 — so `{ i8, float }` is i8@0, f32@2, size 6
    // (round_up(1 + 4, 2) = 6), exactly like the i32 case.
    const S: &str = r#"
%struct.S = type { i8, float }
@out = global %struct.S zeroinitializer, align 2
"#;
    let m = parse_ll(S);
    assert_eq!(m.globals[0].size, 6, "{{ i8, float }} must be 6 bytes");
}

#[test]
fn parses_float_call_arg_constant() {
    // A float call arg constant (`float noundef 5.000000e-01`) must parse
    // (the real f1.ll tail-call shape — regression: parse_call_arg used to
    // reject non-integer constants).
    const S: &str = r#"
define void @main() {
  %1 = tail call float @fadd(float noundef 5.000000e-01)
  ret void
}
define float @fadd(float %0) {
  %2 = fadd float %0, 0x3F800000
  ret float %2
}
"#;
    let m = parse_ll(S);
    let main = &m.funcs[0];
    match &main.blocks[0].insts[0] {
        Inst::Call(c) => {
            assert_eq!(c.ty, Some(ir::Ty::F32));
            assert_eq!(c.args[0].ty, Some(ir::Ty::F32));
            assert_eq!(c.args[0].val, Val::Const(0.5f32.to_bits() as i64));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

#[test]
fn implicit_entry_block_is_numbered_after_the_unnamed_params() {
    // A phi reaching a loop header from the entry names that block, and the
    // backends key phi copies on the edge, so mislabelling the entry drops the
    // loop counter's initialisation: the loop then starts from whatever the
    // overlaid slot happened to hold.
    const S: &str = r#"
define dso_local noundef ptr @memcpy(ptr noundef returned writeonly %0, ptr nocapture noundef readonly %1, i16 noundef %2) local_unnamed_addr #1 {
  %4 = icmp eq i16 %2, 0
  br i1 %4, label %5, label %6

5:                                                ; preds = %6, %3
  ret ptr %0

6:                                                ; preds = %6, %3
  %7 = phi i16 [ %11, %6 ], [ 0, %3 ]
  %11 = add nuw i16 %7, 1
  %12 = icmp eq i16 %11, %2
  br i1 %12, label %5, label %6
}
"#;
    let m = parse_ll(S);
    let f = &m.funcs[0];
    assert_eq!(f.params.len(), 3);
    let labels: Vec<&str> = f.blocks.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, ["3", "5", "6"]);

    // Every phi predecessor must name a real block, otherwise the edge-keyed
    // phi-copy lookup silently finds nothing.
    let block_labels: Vec<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Phi(p) = i {
                for (_, pred) in &p.incoming {
                    assert!(
                        block_labels.contains(pred),
                        "phi pred {pred} is not a block"
                    );
                }
            }
        }
    }
}

#[test]
fn pointer_select_preserves_inlined_gep_offsets() {
    // The ccp_sel shape: a ptr select over an inlined GEP (offset 4) and a
    // bare global. The offset must survive as a materialized Gep inst, and
    // the select must be flagged pointer-typed.
    let text = r#"
@addrs = dso_local constant [8 x i8] zeroinitializer, align 1
define dso_local ptr @ccp_sel(i8 %0) {
  %2 = icmp eq i8 %0, 1
  %3 = select i1 %2, ptr getelementptr inbounds nuw (i8, ptr @addrs, i16 4), ptr @addrs
  ret ptr %3
}
"#;
    let m = parse_ll(text);
    let f = m
        .funcs
        .iter()
        .find(|f| f.name == "ccp_sel")
        .expect("ccp_sel parsed");
    let mut saw_gep = false;
    let mut saw_select = false;
    for i in &f.blocks[0].insts {
        match i {
            Inst::Gep(g) => {
                assert_eq!(g.k, 4, "inlined select-arm GEP must keep its offset");
                assert!(matches!(&g.base, GepBase::Global(n) if n == "addrs"));
                saw_gep = true;
            }
            Inst::Select(s) => {
                assert!(s.ptr, "ptr select must be flagged");
                assert_eq!(s.ty, ir::Ty::I16);
                saw_select = true;
            }
            _ => {}
        }
    }
    assert!(saw_gep, "the inlined GEP arm must be materialized");
    assert!(saw_select, "the select must parse");
}

#[test]
fn reg_arm_pointer_select_stays_a_value_select() {
    // A ptr select over runtime regs (e.g. strrchr's loop select) is a
    // 2-byte value select, not a pointer fold: no synthesized gep, ptr=false.
    let text = r#"
define dso_local ptr @f(ptr %0, ptr %1, i1 %c) {
  %3 = select i1 %c, ptr %0, ptr %1
  ret ptr %3
}
"#;
    let m = parse_ll(text);
    let f = m.funcs.iter().find(|f| f.name == "f").expect("f found");
    match &f.blocks[0].insts[0] {
        Inst::Select(s) => {
            assert!(!s.ptr, "reg-arm ptr select stays a value select");
            assert_eq!(s.ty, ir::Ty::I16);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn parses_indirect_call_through_function_pointer() {
    // `call void %3(...)` is an indirect call: the callee is an SSA register,
    // not a function name. irparse must keep the numeric register name in
    // `func` (the sigil distinction is what lets the backend lower the two
    // differently, epic-cc#73) and leave `callees` empty for legalize to fill.
    let src = r#"
define dso_local void @main() {
  %1 = load i8, ptr @sel, align 1
  %2 = icmp eq i8 %1, 0
  %3 = select i1 %2, ptr @f0, ptr @f1
  call void %3()
  ret void
}
"#;
    let m = parse_ll(src);
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .find_map(|i| match i {
            Inst::Call(c) => Some(c),
            _ => None,
        })
        .expect("expected a call");
    assert_eq!(
        call.func, "3",
        "indirect callee keeps the SSA register name"
    );
    assert!(call.callees.is_empty(), "callees filled later by legalize");
    assert_eq!(call.ty, None, "void call");
    assert!(call.args.is_empty());
}

// epic-cc#133: clang folds the HAL's abs idiom to
// `tail call i16 @llvm.abs.i16(i16 %x, i1 false)`. The `i1 false` immarg
// is not an integer token, so parse_call_arg must accept `true`/`false`
// (as Const(1)/Const(0), the same mapping parse_val uses).
const ABS_INTRINSIC: &str = r#"
define i16 @f(i16 %x) {
  %a = tail call i16 @llvm.abs.i16(i16 %x, i1 false)
  ret i16 %a
}
declare i16 @llvm.abs.i16(i16, i1 immarg)
"#;

#[test]
fn parses_abs_intrinsic_i1_false_immarg() {
    let m = parse_ll(ABS_INTRINSIC);
    let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
    match &f.blocks[0].insts[0] {
        Inst::Call(c) => {
            assert_eq!(c.func, "llvm.abs.i16");
            assert_eq!(c.args.len(), 2);
            assert_eq!(c.args[0].val, Val::Reg("x".to_string()));
            assert_eq!(c.args[1].val, Val::Const(0), "i1 false -> Const(0)");
            assert_eq!(c.args[1].ty, Some(Ty::I1));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}
