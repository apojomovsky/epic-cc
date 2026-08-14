use pic14_sim::Pic14;

fn run(words: &[u16]) -> Pic14 {
    let mut p = Pic14::new(words.to_vec());
    p.run(1000);
    p
}

#[test]
fn btfs_skips_when_bit_set() {
    // MOVLW 0x04 ; MOVWF 0x20 ; BTFSS 0x20,2 ; MOVLW 0xAA ; MOVWF 0x21
    let p = run(&[0x3004, 0x00A0, 0x1D20, 0x30AA, 0x00A1]);
    assert_eq!(p.ram()[0x21], 0x04); // bit 2 set -> MOVLW 0xAA skipped, W stays 0x04
}

#[test]
fn sublw_sets_carry_when_no_borrow() {
    // MOVLW 0x01 ; SUBLW 0x02  -> W = 2 - 1 = 1, C set
    let p = run(&[0x3001, 0x3C02]);
    assert_eq!(p.w(), 0x01);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001);
}

#[test]
fn call_and_retlw_roundtrip() {
    // CALL 0x04 (0x2004) -> push return addr 1, jump to 4 ; at 4: RETLW 0x42 -> W=0x42, return to 1
    let mut p = Pic14::new(vec![0x2004, 0x0000, 0x0000, 0x0000, 0x3442]);
    p.run(2);
    assert_eq!(p.w(), 0x42);
    assert_eq!(p.pc(), 0x01);
}
