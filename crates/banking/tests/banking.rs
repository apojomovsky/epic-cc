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

#[test]
fn literal_immediates_may_exceed_0x7f() {
    // ADDLW/MOVLW/... take an 8-bit literal, not a file-register address:
    // 0xFC (= -4) is legal in a literal slot and must not be range-checked.
    let asm = "    MOVLW 0xFF\n    ADDLW 0xFC\n    MOVWF 0x21\n";
    assert_eq!(assign_banks(asm), asm);
}
