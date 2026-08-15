use irparse::parse_ll;
use ir::{CallArg, GepBase, Global, Inst, Val};

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
            assert_eq!(c.args[0], CallArg { ty: Some(ir::Ty::I16), val: Val::Reg("10".to_string()), byval: None, sret: false });
            assert_eq!(c.args[1], CallArg { ty: Some(ir::Ty::I16), val: Val::Reg("13".to_string()), byval: None, sret: false });
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
            assert!(s.ptr.starts_with("%__gep"), "store ptr must reference the synthesized gep reg: {}", s.ptr);
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
        body.iter().any(|i| matches!(i, Inst::Memcpy(m) if m.dst == Val::Global("g1".to_string())
            && m.src == Val::Global("g2".to_string()) && m.len == 4)),
        "memcpy must appear: {body:?}"
    );
    // lifetime.start/end produce no instructions
    assert!(
        !body.iter().any(|i| matches!(i, Inst::Call(c) if c.func.starts_with("llvm.lifetime"))),
        "lifetime calls must be skipped"
    );
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
