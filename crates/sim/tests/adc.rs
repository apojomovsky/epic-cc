//! No-hang ADC and comparator stubs (epic-cc#1391): firmware polling GO
//! terminates with the deterministic stub result, and comparators are off
//! at POR. CCP/TMR1 writes ride along in the ADC programs to prove they
//! are accepted without disturbing the conversion.

use pic14_sim::{Pic14, Pic14e};

// PIC14 poll program: park values in TMR1L/CCP1CON, raise GO, then poll
// `BTFSC GO / GOTO $-1` to SLEEP. Encodings: MOVLW/MOVWF pairs, BSF
// ADCON0,2 (0x151F), BTFSC (0x191F), GOTO 5 (0x2805), SLEEP.
fn pic14_adc_poll() -> Vec<u16> {
    vec![
        0x3055, 0x008E, 0x300F, 0x0097, 0x151F, 0x191F, 0x2805, 0x0063,
    ]
}

#[test]
fn pic14_adc_poll_terminates_with_por_result() {
    let mut p = Pic14::new(pic14_adc_poll());
    p.run(1000);
    assert!(p.halted(), "poll loop reached SLEEP");
    assert_eq!(p.pc(), 7);
    assert_eq!(p.ram()[0x1F] & 0x04, 0, "GO cleared by the stub");
    assert_eq!(p.ram()[0x1E], 0x80, "ADRESH holds the stub result");
    assert_eq!(p.ram()[0x9E], 0x00, "ADRESL holds the stub result");
    assert_eq!(p.ram()[0x0E], 0x55, "TMR1 write accepted");
    assert_eq!(p.ram()[0x17], 0x0F, "CCP1CON write accepted");
}

#[test]
fn pic14_comparator_off_by_default() {
    let p = Pic14::new(vec![0x0000]);
    assert_eq!(p.ram()[0x1F], 0, "ADCON0: ADC off, GO clear");
    assert_eq!(p.ram()[0x9F], 0, "ADCON1: POR pin config");
    assert_eq!(p.ram()[0x9C], 0x07, "CMCON: comparators off");
}

// PIC14E poll program: same shape, BSR-paged (ADCON0 0x9D bank 1, CCP1CON
// 0x293 bank 5, GO is ADCON0 bit 1). MOVLB selects each bank; the poll is
// BSF 0x1D,1 (0x149D), BTFSC (0x189D), GOTO 7 (0x2807), SLEEP.
fn pic14e_adc_poll() -> Vec<u16> {
    vec![
        0x3055, 0x0096, 0x300F, 0x0025, 0x0093, 0x0021, 0x149D, 0x189D, 0x2807, 0x0063,
    ]
}

#[test]
fn pic14e_adc_poll_terminates_with_por_result() {
    let mut p = Pic14e::new(pic14e_adc_poll());
    p.run(1000);
    assert!(p.halted(), "poll loop reached SLEEP");
    assert_eq!(p.pc(), 9);
    assert_eq!(p.ram()[0x9D] & 0x02, 0, "GO cleared by the stub");
    assert_eq!(p.ram()[0x9C], 0x80, "ADRESH holds the stub result");
    assert_eq!(p.ram()[0x9B], 0x00, "ADRESL holds the stub result");
    assert_eq!(p.ram()[0x16], 0x55, "TMR1 write accepted");
    assert_eq!(p.ram()[0x293], 0x0F, "CCP1CON write accepted");
}

#[test]
fn pic14e_comparator_off_by_default() {
    let p = Pic14e::new(vec![0x0000]);
    assert_eq!(p.ram()[0x9D], 0, "ADCON0: ADC off, GO clear");
    assert_eq!(p.ram()[0x9E], 0, "ADCON1: POR pin config");
    assert_eq!(p.ram()[0x111] & 0x80, 0, "C1ON clear");
    assert_eq!(p.ram()[0x113] & 0x80, 0, "C2ON clear");
}
