use pic14_sim::Pic14;

#[test]
fn indf_aliases_fsr() {
    // MOVLW 0x20 ; MOVWF 0x04 (FSR) ; MOVLW 0x55 ; MOVWF 0x00 (INDF) ; MOVF 0x20,W
    let mut p = Pic14::new(vec![0x3020, 0x0084, 0x3055, 0x0080, 0x0820]);
    p.run(1000);
    assert_eq!(p.w(), 0x55); // MOVWF INDF wrote RAM[0x20] via FSR=0x20
}

#[test]
fn movwf_pcl_computed_jump() {
    // idx=2 -> CALL table-reader (word 4) -> ADDLW LOW(table)=6 -> W=8 -> MOVWF PCL
    // jumps to word 8 = RETLW 0x30 (table[2]); RETLW returns to word 2 (SLEEP).
    // Program: 0: MOVLW 0x02  1: CALL 0x04  2: SLEEP  3: NOP
    //          4: ADDLW 0x06  5: MOVWF PCL  6..9: RETLW 0x10/0x20/0x30/0x40
    let mut p = Pic14::new(vec![
        0x3002, 0x2004, 0x0063, 0x0000, 0x3E06, 0x0082, 0x3410, 0x3420, 0x3430, 0x3440,
    ]);
    p.run(1000);
    assert_eq!(p.w(), 0x30); // table[2]
    assert!(p.halted()); // returned to SLEEP
}
