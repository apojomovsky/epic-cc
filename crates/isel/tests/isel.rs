use isel::select;
use ir::parse;
use std::collections::HashMap;

#[test]
fn emits_add_for_in_plus_one() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("in".to_string(), 0x20u8);
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x20, W"));
    assert!(asm.contains("ADDLW 0x01"));
    assert!(asm.contains("MOVWF 0x21"));
}

#[test]
#[should_panic(expected = "only i8 loads supported")]
fn panics_on_non_i8_load() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    %1 = load i16 @in\n    ret void\n");
    select(&m, &HashMap::new());
}
