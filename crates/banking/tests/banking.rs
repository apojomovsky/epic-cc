use banking::assign_banks;

#[test]
fn passes_bank0_asm_through() {
    let asm = "    MOVF 0x20, W\n    MOVWF 0x21\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
#[should_panic]
fn rejects_bank_operand() {
    assign_banks("    MOVF 0x80, W\n");
}
