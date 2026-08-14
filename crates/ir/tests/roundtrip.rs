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
