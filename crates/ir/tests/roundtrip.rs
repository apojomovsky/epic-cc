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
