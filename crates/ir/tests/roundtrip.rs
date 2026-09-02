use ir::{parse, serialize, GepBase};

#[test]
fn roundtrips_a_straight_line_program() {
    let text = "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out); // stable fixed point
    assert!(out.contains("%2 = add i8 %1 1"));
}

#[test]
fn global_type_and_addr_roundtrip() {
    let m = parse("global in i8\nconst out i16 @0x20\nfn main(void) ()\n");
    assert_eq!(m.globals[0].ty, ir::Ty::I8);
    assert_eq!(m.globals[0].addr, None);
    assert_eq!(m.globals[1].ty, ir::Ty::I16);
    assert_eq!(m.globals[1].addr, Some(0x20));
    let out = serialize(&m);
    assert!(out.contains("global in i8"));
    assert!(out.contains("const out i16 @0x20"));
    // stable fixed point
    assert_eq!(serialize(&parse(&out)), out);
}

#[test]
fn explicit_global_addr_past_0xff_roundtrips() {
    // Issue #9: Global.addr used to be a u8, so an explicit .ll address past
    // 0xFF (e.g. a bank-2 GPR address like 0x150) panicked in parse_addr
    // instead of round-tripping like any other explicit address.
    let m = parse("global g i16 @0x150\n");
    assert_eq!(m.globals[0].addr, Some(0x150));
    let out = serialize(&m);
    assert!(
        out.contains("global g i16 @0x150"),
        "missing wide addr\n---\n{out}"
    );
    assert_eq!(serialize(&parse(&out)), out, "stable fixed point");
}

#[test]
fn gep_and_sized_globals_roundtrip() {
    let text = "global ram i8 @0x25\nconst table i8\nfn main(void) ()\n  block entry:\n    %p = gep @ram +0 +1*%3\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // gep line round-trips verbatim
    assert!(
        out.contains("%p = gep @ram +0 +1*%3"),
        "missing gep line\n---\n{out}"
    );
    // sized global keeps its address
    assert!(
        out.contains("global ram i8 @0x25"),
        "missing global addr\n---\n{out}"
    );
    // const global carries no @addr in the canonical text
    assert!(
        out.contains("const table i8\n"),
        "missing const line\n---\n{out}"
    );
    assert!(
        !out.contains("const table i8 @"),
        "const must serialize without an address\n---\n{out}"
    );
    // parsed scalar global sizes default from the type (widened to u16)
    assert_eq!(m.globals[0].size, u16::from(ir::Ty::I8.bytes()));
    assert_eq!(m.globals[1].size, u16::from(ir::Ty::I8.bytes()));
    assert_eq!(m.globals[0].bytes, Vec::<u8>::new());
    // size/bytes are struct-only metadata: a Global constructed with them keeps them
    let g = ir::Global {
        name: "ram".into(),
        ty: ir::Ty::I8,
        is_const: false,
        addr: Some(0x25),
        size: 8,
        bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
        refs: Vec::new(),
    };
    assert_eq!(g.size, 8);
    assert_eq!(g.bytes, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn roundtrips_all_icmp_predicates_and_sext() {
    let preds = [
        "eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge",
    ];
    let mut insts = String::new();
    for (i, p) in preds.iter().enumerate() {
        insts.push_str(&format!("    %c{i} = icmp {p} i8 %a %b\n"));
    }
    insts.push_str("    %s = sext i8 %v to i16\n");
    let text = format!("fn main(void) ()\n  block entry:\n{insts}    ret void\n");
    let m = parse(&text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // every predicate serializes verbatim
    for (i, p) in preds.iter().enumerate() {
        assert!(
            out.contains(&format!("%c{i} = icmp {p} i8 %a %b")),
            "missing {p}\n---\n{out}"
        );
    }
    // sext serializes canonically
    assert!(
        out.contains("%s = sext i8 %v to i16"),
        "missing sext\n---\n{out}"
    );
}

#[test]
#[should_panic(expected = "unsupported icmp predicate")]
fn rejects_unknown_icmp_predicate() {
    parse("fn main(void) ()\n  block entry:\n    %c = icmp foo i8 %a %b\n    ret void\n");
}

#[test]
fn roundtrips_control_flow_call_and_cast() {
    let text = "fn main(void) ()\n\
  block main:\n\
    %1 = load i8 @in\n\
    %2 = zext i8 %1 to i16\n\
    %3 = icmp eq i16 %2 0\n\
    br i1 %3 6 8\n\
  block 6:\n\
    %5 = trunc i16 %14 to i8\n\
    br 8\n\
  block 8:\n\
    %9 = phi i16 0 main %15 main_L8\n\
    %10 = load i16 @out\n\
    %11 = icmp eq i16 %10 0\n\
    %12 = icmp eq i16 %11 0\n\
    %13 = select i1 %12 i16 100 i16 %9\n\
    %14 = call i16 @add(i16 %10, i16 %13)\n\
    %15 = icmp ne i16 %14 0\n\
    call void @f()\n\
    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // key canonical lines survive verbatim
    for line in [
        "%9 = phi i16 0 main %15 main_L8",
        "br i1 %3 6 8",
        "%14 = call i16 @add(i16 %10, i16 %13)",
        "call void @f()",
        "%2 = zext i8 %1 to i16",
        "%5 = trunc i16 %14 to i8",
        "%12 = icmp eq i16 %11 0",
        "%13 = select i1 %12 i16 100 i16 %9",
    ] {
        assert!(
            out.contains(line),
            "missing canonical line: {line}\n---\n{out}"
        );
    }
}

#[test]
fn roundtrips_indirect_call_callees() {
    // An indirect call's candidate list round-trips through the canonical
    // text (`callees <f0> <f1> ...`), and a direct call carries no suffix.
    let text = "fn main(void) ()\n  block entry:\n    %1 = call i16 %3(i16 3, i16 4) callees add mul\n    call void %5() callees f0 f1\n    call void @f()\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    assert!(
        out.contains("%1 = call i16 %3(i16 3, i16 4) callees add mul"),
        "missing indirect call callees\n---\n{out}"
    );
    assert!(
        out.contains("call void %5() callees f0 f1"),
        "missing void indirect call callees\n---\n{out}"
    );
    assert!(
        out.contains("call void @f()"),
        "direct call must keep no callees suffix\n---\n{out}"
    );
    // The parsed callees lists are populated.
    let calls: Vec<&ir::Call> = m.funcs[0]
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| match i {
            ir::Inst::Call(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(calls[0].callees, vec!["add".to_string(), "mul".to_string()]);
    assert_eq!(calls[1].callees, vec!["f0".to_string(), "f1".to_string()]);
    assert!(calls[2].callees.is_empty(), "direct call has no callees");
}

#[test]
fn roundtrips_runtime_length_memcpy() {
    // Issue #4: the register-length form (`memcpy dst src %n`) round-trips
    // as MemLen::Reg — the counted-loop form — not as a const parse error.
    let text =
        "global a i8\nfn f(i16) (n=i16)\n  block entry:\n    memcpy @a %1 %n\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    assert!(
        out.contains("memcpy @a %1 %n"),
        "runtime-len memcpy\n---\n{out}"
    );
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // the parsed len is the register form
    match &m2.funcs[0].blocks[0].insts[0] {
        ir::Inst::Memcpy(mm) => {
            assert_eq!(
                mm.len,
                ir::MemLen::Reg(ir::Val::Reg("n".to_string())),
                "len must round-trip as the register form"
            );
        }
        other => panic!("expected Memcpy, got {other:?}"),
    }
}

#[test]
fn roundtrips_reworked_gep_alloca_memcpy_and_params() {
    let text = "global a i8\nfn f(i16) (x=i8, s=sret)\n  block entry:\n    %p = gep %s +2 +2*%x\n    %1 = alloca 4\n    memcpy @a %1 4\n    ret void\n";
    let m = parse(text);
    assert_eq!(m.funcs[0].params.len(), 2);
    assert_eq!(m.funcs[0].params[0].name, "x");
    assert_eq!(m.funcs[0].params[0].width, 1); // i8 scalar width on canonical text
    assert_eq!(m.funcs[0].params[1].name, "s");
    assert!(m.funcs[0].params[1].sret);

    let out = serialize(&m);
    assert!(
        out.contains("fn f(i16) (x=i8, s=sret)"),
        "params header\n---\n{out}"
    );
    assert!(out.contains("%p = gep %s +2 +2*%x"), "gep\n---\n{out}");
    assert!(out.contains("%1 = alloca 4"), "alloca\n---\n{out}");
    assert!(out.contains("memcpy @a %1 4"), "memcpy\n---\n{out}");

    // stable fixed point
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);

    // struct: fields round-trip
    let m3 = parse(&out);
    match &m3.funcs[0].blocks[0].insts[0] {
        ir::Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Reg("s".to_string()));
            assert_eq!(g.k, 2);
            assert_eq!(g.terms, vec![(2, "x".to_string())]);
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

#[test]
fn roundtrips_byval_param_and_call_arg() {
    let text = "global a i8\nfn f(i8) (p=byval4)\n  block entry:\n    %r = call i8 @g(i8 %1, byval4 %p)\n    ret void\n";
    let m = parse(text);
    assert_eq!(m.funcs[0].params[0].name, "p");
    assert_eq!(m.funcs[0].params[0].byval, Some(4));
    assert_eq!(m.funcs[0].params[0].width, 4);
    let out = serialize(&m);
    assert!(
        out.contains("fn f(i8) (p=byval4)"),
        "byval param header\n---\n{out}"
    );
    assert!(
        out.contains("%r = call i8 @g(i8 %1, byval4 %p)"),
        "call args\n---\n{out}"
    );
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    match &m2.funcs[0].blocks[0].insts[0] {
        ir::Inst::Call(c) => {
            assert_eq!(c.args.len(), 2);
            assert_eq!(c.args[0].ty, Some(ir::Ty::I8));
            assert_eq!(c.args[1].byval, Some(4));
            assert_eq!(c.args[1].val, ir::Val::Reg("p".to_string()));
        }
        other => panic!("expected Call, got {other:?}"),
    }
}

#[test]
fn scalar_param_widths_roundtrip() {
    // Regression: canonical fn-header text used to drop scalar widths — an
    // i16 param re-parsed as width 1, silently undersizing its slot in the
    // stage-bin pipeline (irparse -> wholeprog -> legalize -> alloc -> isel).
    // Scalar params must round-trip their width.
    let text = "fn f(i16) (x=i16, y=i8, p=byval4, s=sret)\n  block entry:\n    ret void\n";
    let m = parse(text);
    assert_eq!(m.funcs[0].params[0].width, 2); // i16 -> width 2
    assert_eq!(m.funcs[0].params[1].width, 1); // i8 -> width 1
    let out = serialize(&m);
    assert_eq!(
        out, text,
        "scalar widths must serialize back to typed params\n---\n{out}"
    );
    // re-parse: the i16 scalar keeps width 2 (re-parsed as 1 before the fix)
    let m2 = parse(&out);
    assert_eq!(
        m2.funcs[0].params[0].width, 2,
        "i16 scalar param must re-parse with width 2"
    );
    assert_eq!(serialize(&m2), out);
}

#[test]
fn roundtrips_new_binops_and_freeze() {
    // Milestone 8: the eight mul/div/rem/shift binops + freeze must
    // round-trip their canonical text exactly.
    let text = "global in i16\nfn main(void) ()\n  block entry:\n\
    %0 = load i16 @in\n\
    %1 = mul i16 %0, 7\n\
    %2 = udiv i16 %1, 7\n\
    %3 = urem i16 %2, 5\n\
    %4 = sdiv i16 %3, -3\n\
    %5 = srem i16 %4, 3\n\
    %6 = shl i16 %5, 3\n\
    %7 = lshr i16 %6, 1\n\
    %8 = ashr i16 %7, 2\n\
    %9 = freeze i16 %8\n\
    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // each new opcode serializes to its canonical opcode name
    for line in [
        "%1 = mul i16 %0 7",
        "%2 = udiv i16 %1 7",
        "%3 = urem i16 %2 5",
        "%4 = sdiv i16 %3 -3",
        "%5 = srem i16 %4 3",
        "%6 = shl i16 %5 3",
        "%7 = lshr i16 %6 1",
        "%8 = ashr i16 %7 2",
        "%9 = freeze i16 %8",
    ] {
        assert!(
            out.contains(line),
            "missing canonical line: {line}\n---\n{out}"
        );
    }
}

#[test]
fn gep_base_global_roundtrips() {
    let text = "global a i8\nfn main(void) ()\n  block entry:\n    %p = gep @a +3\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    assert!(
        out.contains("%p = gep @a +3"),
        "const-offset gep\n---\n{out}"
    );
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    match &m2.funcs[0].blocks[0].insts[0] {
        ir::Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("a".to_string()));
            assert_eq!(g.k, 3);
            assert!(g.terms.is_empty());
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

#[test]
fn roundtrips_runtime_inttoptr() {
    // epic-cc#117: a runtime integer address becoming a pointer VALUE keeps
    // its own instruction (distinct from `zext`, whose i16->i16 shape is a
    // plain value copy) so iselcore can seed the dst as an indirect pointer.
    let text = "fn main(void) ()\n  block entry:\n    %p = inttoptr i16 %a to i16\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    assert!(
        out.contains("%p = inttoptr i16 %a to i16"),
        "inttoptr inst\n---\n{out}"
    );
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out, "stable fixed point");
    match &m2.funcs[0].blocks[0].insts[0] {
        ir::Inst::IntToPtr(p) => {
            assert_eq!(p.dst, "p");
            assert_eq!(p.from, ir::Ty::I16);
            assert_eq!(p.val, ir::Val::Reg("a".to_string()));
            assert_eq!(p.to, ir::Ty::I16);
        }
        other => panic!("expected IntToPtr, got {other:?}"),
    }
}

#[test]
fn roundtrips_isr_marker() {
    // Milestone 13: the interrupt marker `[isr]` sits between the ret group
    // and the params group: `fn isr(void) [isr] ()`. It must round-trip and
    // be absent on ordinary functions.
    let text = "fn isr(void) [isr] ()\n  block entry:\n    ret void\n";
    let m = parse(text);
    assert!(m.funcs[0].isr, "isr marker must parse to Func.isr == true");
    let out = serialize(&m);
    assert!(
        out.contains("fn isr(void) [isr] ()"),
        "isr marker header\n---\n{out}"
    );
    let m2 = parse(&out);
    assert!(m2.funcs[0].isr, "isr marker must round-trip");
    assert_eq!(serialize(&m2), out, "stable fixed point");
    // a plain function carries no marker
    let m3 = parse("fn main(void) ()\n  block entry:\n    ret void\n");
    assert!(!m3.funcs[0].isr);
    assert!(
        !serialize(&m3).contains("[isr]"),
        "no marker on a non-isr fn"
    );
}

#[test]
fn roundtrips_literal_ptr_load_store() {
    // Milestone 13: inttoptr constant pointers serialize as the literal ptr
    // form `0x<K>` (distinct from `@global` / `%reg`).
    let text = "fn main(void) ()\n  block entry:\n    %1 = load i8 0x06\n    store i8 85 0x06\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    assert!(
        out.contains("%1 = load i8 0x06"),
        "literal ptr load\n---\n{out}"
    );
    assert!(
        out.contains("store i8 85 0x06"),
        "literal ptr store\n---\n{out}"
    );
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out, "stable fixed point");
    match &m2.funcs[0].blocks[0].insts[0] {
        ir::Inst::Load(l) => assert_eq!(l.ptr, "0x06"),
        other => panic!("expected Load, got {other:?}"),
    }
}

#[test]
fn roundtrips_i32_ops() {
    // Milestone 12: i32 type flows through every type position the serializer
    // and parser touch — binop, icmp, casts, and a sized scalar param.
    let text = "global in i32\nfn f(i32) (x=i32)\n  block entry:\n    %1 = add i32 %x, 2\n    %2 = icmp ult i32 %1, 10\n    %3 = zext i8 %1 to i32\n    %4 = trunc i32 %1 to i8\n    %5 = sext i16 %1 to i32\n    store i32 %5 @in\n    ret void\n";
    let m = parse(text);
    assert_eq!(
        m.funcs[0].params[0].width, 4,
        "i32 scalar param must carry width 4"
    );
    let out = serialize(&m);
    // stable fixed point
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    for line in [
        "%1 = add i32 %x 2",
        "%2 = icmp ult i32 %1 10",
        "%3 = zext i8 %1 to i32",
        "%4 = trunc i32 %1 to i8",
        "%5 = sext i16 %1 to i32",
        "fn f(i32) (x=i32)",
        "store i32 %5 @in",
        "global in i32",
    ] {
        assert!(
            out.contains(line),
            "missing canonical line: {line}\n---\n{out}"
        );
    }
}

#[test]
fn roundtrips_float_insts_and_constants() {
    // Milestone 15: the float instruction family round-trips its canonical
    // text. An f32 constant serializes as its 32-bit bit pattern as a
    // decimal integer (e.g. 1.0f = 0x3F800000 = 1065353216), which re-parses
    // back to the same `Val::Const`.
    let text = "global in float\nfn f(float) (a=float)\n  block entry:\n\
    %1 = fadd float %a %b\n\
    %2 = fsub float %1 1065353216\n\
    %3 = fmul float %2 %a\n\
    %4 = fdiv float %3 %1\n\
    %5 = fcmp olt float %4 %a\n\
    %6 = fptosi float %a to i16\n\
    %7 = fptoui float %a to i32\n\
    %8 = sitofp i16 %6 to float\n\
    %9 = uitofp i32 %7 to float\n\
    %10 = fpext float %8 to float\n\
    %11 = fptrunc float %9 to float\n\
    ret float %11\n";
    let m = parse(text);
    assert_eq!(m.globals[0].ty, ir::Ty::F32, "float global type");
    assert_eq!(m.globals[0].size, 4, "float global is 4 bytes");
    assert_eq!(
        m.funcs[0].params[0].width, 4,
        "float scalar param is width 4"
    );
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out, "stable fixed point\n---\n{out}");
    for line in [
        "global in float",
        "fn f(float) (",
        // a width-4 scalar param serializes as `i32` (the Param type carries
        // only the width, so float is not distinguished — the slot is 4 bytes
        // either way, matching the i32 precedent).
        "a=i32",
        "%1 = fadd float %a %b",
        "%2 = fsub float %1 1065353216",
        "%3 = fmul float %2 %a",
        "%4 = fdiv float %3 %1",
        "%5 = fcmp olt float %4 %a",
        "%6 = fptosi float %a to i16",
        "%7 = fptoui float %a to i32",
        "%8 = sitofp i16 %6 to float",
        "%9 = uitofp i32 %7 to float",
        "%10 = fpext float %8 to float",
        "%11 = fptrunc float %9 to float",
        "ret float %11",
    ] {
        assert!(
            out.contains(line),
            "missing canonical line: {line}\n---\n{out}"
        );
    }
}
