use pic14_sim::Pic14;

fn run(words: &[u16], w_value: u8, ram20: u8) -> Pic14 {
    // Seed 0x22 (scratch) and 0x20 so W can be loaded via MOVF (no MOVLW yet).
    let mut p = Pic14::new(words.to_vec());
    p.ram_mut()[0x22] = w_value;
    p.ram_mut()[0x20] = ram20;
    p.run(1000);
    p
}

#[test]
fn movwf_then_movf_roundtrip() {
    // MOVF 0x22,W (W=0x2A) ; MOVWF 0x20 ; MOVF 0x20,W ; MOVWF 0x21
    let p = run(&[0x0822, 0x00A0, 0x0820, 0x00A1], 0x2A, 0x00);
    assert_eq!(p.ram()[0x21], 0x2A);
}

#[test]
fn addwf_carries_and_zero() {
    // MOVF 0x22,W (W=1) ; ADDWF 0x20,W (1 + 0xFF = 0x00, C) ; MOVWF 0x21
    let p = run(&[0x0822, 0x0720, 0x00A1], 0x01, 0xFF);
    assert_eq!(p.ram()[0x21], 0x00); // FF + 01 wraps to 0
    assert_eq!(p.ram()[0x03] & 0b001, 0b001); // carry set
    assert_eq!(p.ram()[0x03] & 0b100, 0b100); // zero set
}
