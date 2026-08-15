use irparse::parse_ll;
use ir::{Global, Inst, Val};

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
        Global { name, ty, is_const, size, bytes, addr } => {
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
        Global { name, ty, is_const, size, bytes, addr } => {
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

    // %p = getelementptr i8, ptr @ram, i16 %3  -> base ram, offset %3
    match &body[1] {
        Inst::Gep(g) => {
            assert_eq!(g.dst, "p");
            assert_eq!(g.base, "ram");
            assert_eq!(g.offset, Val::Reg("3".to_string()));
        }
        other => panic!("expected Gep, got {other:?}"),
    }

    // %q = getelementptr [4 x i8], ptr @table, i16 0, i16 %3 -> base table, offset %3
    match &body[2] {
        Inst::Gep(g) => {
            assert_eq!(g.dst, "q");
            assert_eq!(g.base, "table");
            assert_eq!(g.offset, Val::Reg("3".to_string()));
        }
        other => panic!("expected Gep, got {other:?}"),
    }
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
            assert_eq!(c.args[0], (ir::Ty::I16, Val::Reg("10".to_string())));
            assert_eq!(c.args[1], (ir::Ty::I16, Val::Reg("13".to_string())));
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

    for (idx, p) in ["eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge"]
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
