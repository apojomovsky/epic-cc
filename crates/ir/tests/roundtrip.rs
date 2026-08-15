use ir::{parse, serialize};

#[test]
fn roundtrips_a_straight_line_program() {
    let text = "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out); // stable fixed point
    assert!(out.contains("%2 = add i8 %1 1"));
}

#[test]
fn global_type_and_addr_roundtrip() {
    let m = parse("global in i8\nconst out i16 @0x20\nfn main() -> void\n");
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
fn gep_and_sized_globals_roundtrip() {
    let text = "global ram i8 @0x25\nconst table i8\nfn main() -> void\n  block entry:\n    %p = gep @ram %3\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // gep line round-trips verbatim
    assert!(out.contains("%p = gep @ram %3"), "missing gep line\n---\n{out}");
    // sized global keeps its address
    assert!(out.contains("global ram i8 @0x25"), "missing global addr\n---\n{out}");
    // const global carries no @addr in the canonical text
    assert!(out.contains("const table i8\n"), "missing const line\n---\n{out}");
    assert!(!out.contains("const table i8 @"), "const must serialize without an address\n---\n{out}");
    // parsed scalar global sizes default from the type
    assert_eq!(m.globals[0].size, ir::Ty::I8.bytes());
    assert_eq!(m.globals[1].size, ir::Ty::I8.bytes());
    assert_eq!(m.globals[0].bytes, Vec::<u8>::new());
    // size/bytes are struct-only metadata: a Global constructed with them keeps them
    let g = ir::Global {
        name: "ram".into(),
        ty: ir::Ty::I8,
        is_const: false,
        addr: Some(0x25),
        size: 8,
        bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };
    assert_eq!(g.size, 8);
    assert_eq!(g.bytes, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn roundtrips_all_icmp_predicates_and_sext() {
    let preds = ["eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge"];
    let mut insts = String::new();
    for (i, p) in preds.iter().enumerate() {
        insts.push_str(&format!("    %c{i} = icmp {p} i8 %a %b\n"));
    }
    insts.push_str("    %s = sext i8 %v to i16\n");
    let text = format!("fn main() -> void\n  block entry:\n{insts}    ret void\n");
    let m = parse(&text);
    let out = serialize(&m);
    // stable fixed point: parse -> serialize -> parse -> serialize
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out);
    // every predicate serializes verbatim
    for (i, p) in preds.iter().enumerate() {
        assert!(out.contains(&format!("%c{i} = icmp {p} i8 %a %b")), "missing {p}\n---\n{out}");
    }
    // sext serializes canonically
    assert!(out.contains("%s = sext i8 %v to i16"), "missing sext\n---\n{out}");
}

#[test]
#[should_panic(expected = "unsupported icmp predicate")]
fn rejects_unknown_icmp_predicate() {
    parse("fn main() -> void\n  block entry:\n    %c = icmp foo i8 %a %b\n    ret void\n");
}

#[test]
fn roundtrips_control_flow_call_and_cast() {
    let text = "fn main() -> void\n\
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
        assert!(out.contains(line), "missing canonical line: {line}\n---\n{out}");
    }
}
