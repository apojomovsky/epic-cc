use pic14_sim::Pic14;

// Instruction-word encodings used below:
//   MOVLW k  = 0x3000 | k
//   MOVWF f  = 0x0080 | f          (d=1, op6=0x00)
//   MOVF f,W = 0x0800 | f          (d=0, op6=0x08)
//   BSF f,b  = 0x1400 | (b<<7) | f
//   BCF f,b  = 0x1000 | (b<<7) | f
// STATUS = 0x03, RP0 = bit 5, RP1 = bit 6, IRP = bit 7.
const BSF_RP0: u16 = 0x1400 | (5 << 7) | 0x03; // BSF STATUS,5 -> bank 1
const BCF_RP0: u16 = 0x1000 | (5 << 7) | 0x03; // BCF STATUS,5 -> bank 0
const BSF_RP1: u16 = 0x1400 | (6 << 7) | 0x03; // BSF STATUS,6 -> bank 2
const BSF_IRP: u16 = 0x1400 | (7 << 7) | 0x03; // BSF STATUS,7 -> IRP=1
const BCF_IRP: u16 = 0x1000 | (7 << 7) | 0x03; // BCF STATUS,7 -> IRP=0

#[test]
fn banks_are_isolated() {
    // Set bank 1 (RP0=1), write 0x55 to f=0x20 (physical 0xA0), drop back to
    // bank 0, then read f=0x20 -> must see physical 0x20 (0), not 0xA0.
    let mut p = Pic14::new(vec![
        BSF_RP0,
        0x3055, // MOVLW 0x55
        0x00A0, // MOVWF 0x20
        BCF_RP0,
        0x0820, // MOVF 0x20,W
    ]);
    p.run(1000);
    assert_eq!(p.w(), 0x00);           // read bank 0 cell (unwritten)
    assert_eq!(p.ram()[0xA0], 0x55);   // bank 1 cell holds the write
    assert_eq!(p.ram()[0x20], 0x00);   // bank 0 cell untouched
}

#[test]
fn common_region_is_mirrored_across_banks() {
    // Write f=0x70 in bank 0, then read f=0x70 while in bank 1 (RP=01) -> same.
    let mut p = Pic14::new(vec![
        0x3077, // MOVLW 0x77
        0x00F0, // MOVWF 0x70
        BSF_RP0,
        0x0870, // MOVF 0x70,W
    ]);
    p.run(1000);
    assert_eq!(p.w(), 0x77);
}

#[test]
fn banksel_end_to_end() {
    // Bank 1: write 0x11 to f=0x21 (physical 0xA1). Bank 0: write 0x22 to
    // f=0x20 (physical 0x20). Both cells must be independent.
    let mut p = Pic14::new(vec![
        BSF_RP0,
        0x3011, // MOVLW 0x11
        0x00A1, // MOVWF 0x21
        BCF_RP0,
        0x3022, // MOVLW 0x22
        0x00A0, // MOVWF 0x20
    ]);
    p.run(1000);
    assert_eq!(p.ram()[0xA1], 0x11);
    assert_eq!(p.ram()[0x20], 0x22);
    assert_eq!(p.ram()[0xA0], 0x00); // bank 1 physical cell never written
}

#[test]
fn rp1_selects_bank_2() {
    // RP1=1 (bank 2): write f=0x20 -> physical 0x120, distinct from bank 0/1.
    let mut p = Pic14::new(vec![
        BSF_RP1,
        0x3033, // MOVLW 0x33
        0x00A0, // MOVWF 0x20
    ]);
    p.run(1000);
    assert_eq!(p.ram()[0x120], 0x33);
    assert_eq!(p.ram()[0x20], 0x00);
    assert_eq!(p.ram()[0xA0], 0x00);
}

#[test]
fn rp0_rp1_select_bank_3() {
    // RP0=1 + RP1=1 (bank 3): write f=0x20 -> physical 0x1A0.
    let mut p = Pic14::new(vec![
        BSF_RP1,
        BSF_RP0,
        0x3044, // MOVLW 0x44
        0x00A0, // MOVWF 0x20
    ]);
    p.run(1000);
    assert_eq!(p.ram()[0x1A0], 0x44);
    assert_eq!(p.ram()[0x20], 0x00); // bank 0 cell untouched
    assert_eq!(p.ram()[0xA0], 0x00); // bank 1 cell untouched
    assert_eq!(p.ram()[0x120], 0x00); // bank 2 cell untouched
}

#[test]
fn indf_uses_irp_for_upper_half() {
    // FSR=0x20. With IRP=1, MOVWF INDF hits physical 0x120; with IRP=0 it hits
    // physical 0x20.
    let mut p = Pic14::new(vec![
        0x3020, // MOVLW 0x20
        0x0084, // MOVWF FSR (0x04)
        BSF_IRP,
        0x3099, // MOVLW 0x99
        0x0080, // MOVWF INDF (0x00)
        BCF_IRP,
        0x30AA, // MOVLW 0xAA
        0x0080, // MOVWF INDF (0x00)
    ]);
    p.run(1000);
    assert_eq!(p.ram()[0x120], 0x99);
    assert_eq!(p.ram()[0x20], 0xAA);
}

#[test]
fn indf_common_region_ignores_irp() {
    // FSR=0x70 with IRP=1: common region, so the write lands at physical 0x70
    // (not 0x170).
    let mut p = Pic14::new(vec![
        0x3070, // MOVLW 0x70
        0x0084, // MOVWF FSR (0x04)
        BSF_IRP,
        0x3055, // MOVLW 0x55
        0x0080, // MOVWF INDF (0x00)
    ]);
    p.run(1000);
    assert_eq!(p.ram()[0x70], 0x55);
    assert_eq!(p.ram()[0x170], 0x00);
}
