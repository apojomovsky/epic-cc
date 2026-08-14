use ir::{parse, serialize};

#[test]
fn roundtrips_a_straight_line_program() {
    let text = "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out); // stable fixed point
    assert!(out.contains("%2 = add i8 %1, 1"));
}
