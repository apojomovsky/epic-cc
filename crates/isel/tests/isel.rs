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
fn store_const_emits_movlw_not_movf() {
    let m = parse("global out i8\nfn main() -> void\n  block entry:\n    store i8 5 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVLW 0x05"), "expected MOVLW for const store:\n{asm}");
    assert!(asm.contains("MOVWF 0x21"), "expected MOVWF to @out:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
fn add_const_lhs_uses_addlw() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %x = add i8 5, %1\n    store i8 %x @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("in".to_string(), 0x20u8);
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("ADDLW 0x05"), "const-LHS add should use the ADDLW path:\n{asm}");
    assert!(!asm.contains("ADDWF 0x05"), "const must not be read as a file register:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
#[should_panic(expected = "only i8 loads supported")]
fn panics_on_non_i8_load() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    %1 = load i16 @in\n    ret void\n");
    select(&m, &HashMap::new());
}
