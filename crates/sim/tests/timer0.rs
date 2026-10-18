//! Timer0 on PIC14 and PIC14E: prescaled TMR0 advance, overflow flag,
//! and the T0IE vector path. The model is opt-in (`set_timer0_enabled`),
//! internal clock only (T0CS set idles), with the rate from live
//! OPTION_REG state (PSA set bypasses, else PS2:0 selects 2 to 256).
use pic14_sim::{
    Pic14, Pic14e, GIE, INTE, INTF, PIC14E_OPTION_REG, PIC14E_TMR0, PIC14_OPTION_REG, PIC14_T0IE,
    PIC14_T0IF, PIC14_TMR0, VECTOR,
};

const INTCON: usize = 0x0B;
const RETFIE: u16 = 0x0009;

/// NOPs with a RETFIE at the word-4 vector.
fn prog_with_isr() -> Vec<u16> {
    let mut prog = vec![0u16; 64];
    prog[4] = RETFIE;
    prog
}

#[test]
fn pic14_advances_by_cycles_over_prescaler() {
    let mut p = Pic14::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x00; // internal clock, PSA to TMR0, 1:2
    p.run(8);
    assert_eq!(p.ram()[PIC14_TMR0], 4, "8 cycles at 1:2 advance TMR0 by 4");
    assert_eq!(p.ram()[INTCON] & PIC14_T0IF, 0, "no overflow yet");
}

#[test]
fn pic14_overflow_sets_t0if() {
    let mut p = Pic14::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14_TMR0] = 0xFF;
    p.run(2);
    assert_eq!(p.ram()[PIC14_TMR0], 0, "0xFF ticks to 0x00");
    assert_ne!(p.ram()[INTCON] & PIC14_T0IF, 0, "overflow sets T0IF");
    assert!(p.interrupt_pending(), "overflow latches the Timer0 request");
}

#[test]
fn pic14_overflow_vectors_on_gie_and_t0ie() {
    let mut p = Pic14::new(prog_with_isr());
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14_TMR0] = 0xFF;
    p.ram_mut()[INTCON] = GIE | PIC14_T0IE;
    p.run(3);
    assert_eq!(p.pc(), VECTOR, "overflow with GIE+T0IE takes the vector");
    assert_eq!(p.cycles(), 4, "two NOP cycles plus the 2-cycle entry");
    assert!(!p.interrupt_pending(), "the latch is consumed on entry");
}

#[test]
fn pic14_timer_stays_off_unless_enabled() {
    let mut p = Pic14::new(vec![0u16; 64]);
    p.ram_mut()[PIC14_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14_TMR0] = 0xFF;
    p.run(8);
    assert_eq!(p.ram()[PIC14_TMR0], 0xFF, "model off: TMR0 untouched");
    assert_eq!(p.ram()[INTCON] & PIC14_T0IF, 0, "model off: no flag");
    assert!(!p.interrupt_pending(), "model off: nothing latched");
}

#[test]
fn pic14_external_clock_idles() {
    let mut p = Pic14::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x20; // T0CS: external T0CKI, out of scope
    p.run(8);
    assert_eq!(p.ram()[PIC14_TMR0], 0, "external mode never ticks");
}

#[test]
fn pic14_psa_set_bypasses_the_prescaler() {
    let mut p = Pic14::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x08; // PSA: prescaler to WDT, TMR0 1:1
    p.run(3);
    assert_eq!(p.ram()[PIC14_TMR0], 3, "1:1 advances one per cycle");
}

#[test]
fn pic14_option_instruction_loads_option_reg() {
    // MOVLW 0xA5; OPTION (0x0062): W -> OPTION_REG.
    let mut p = Pic14::new(vec![0x30A5, 0x0062, 0x0000]);
    p.run(2);
    assert_eq!(p.ram()[PIC14_OPTION_REG], 0xA5, "OPTION stores W");
}

#[test]
fn pic14_int0_still_vectors_on_inte_only() {
    let mut p = Pic14::new(prog_with_isr());
    p.set_timer0_enabled(true);
    p.ram_mut()[INTCON] = GIE | INTE;
    p.request_interrupt();
    p.run(1);
    assert_eq!(p.pc(), VECTOR, "INT0 request vectors on INTE");
}

#[test]
fn pic14_sources_do_not_cross_trigger() {
    // An INT0 request with only T0IE enabled stays pending: each latch
    // keys off its own enable.
    let mut p = Pic14::new(prog_with_isr());
    p.set_timer0_enabled(true);
    p.ram_mut()[INTCON] = GIE | PIC14_T0IE;
    p.request_interrupt();
    p.run(2);
    assert_ne!(p.pc(), VECTOR, "INT0 must not vector on T0IE");
    assert!(p.interrupt_pending(), "still latched without INTE");
    assert_eq!(p.ram()[INTCON] & INTF, INTF, "INTF untouched by the timer");
}

#[test]
fn pic14e_advances_by_cycles_over_prescaler() {
    let mut p = Pic14e::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x00; // internal clock, PSA to TMR0, 1:2
    p.run(8);
    assert_eq!(p.ram()[PIC14E_TMR0], 4, "8 cycles at 1:2 advance TMR0 by 4");
    assert_eq!(p.ram()[INTCON] & PIC14_T0IF, 0, "no overflow yet");
}

#[test]
fn pic14e_overflow_sets_t0if() {
    let mut p = Pic14e::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14E_TMR0] = 0xFF;
    p.run(2);
    assert_eq!(p.ram()[PIC14E_TMR0], 0, "0xFF ticks to 0x00");
    assert_ne!(p.ram()[INTCON] & PIC14_T0IF, 0, "overflow sets T0IF");
    assert!(p.interrupt_pending(), "overflow latches the Timer0 request");
}

#[test]
fn pic14e_overflow_vectors_on_gie_and_t0ie() {
    let mut p = Pic14e::new(prog_with_isr());
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14E_TMR0] = 0xFF;
    p.ram_mut()[INTCON] = GIE | PIC14_T0IE;
    p.run(3);
    assert_eq!(p.pc(), VECTOR, "overflow with GIE+T0IE takes the vector");
    assert_eq!(p.cycles(), 4, "two NOP cycles plus the 2-cycle entry");
    assert!(!p.interrupt_pending(), "the latch is consumed on entry");
}

#[test]
fn pic14e_timer_stays_off_unless_enabled() {
    let mut p = Pic14e::new(vec![0u16; 64]);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x00;
    p.ram_mut()[PIC14E_TMR0] = 0xFF;
    p.run(8);
    assert_eq!(p.ram()[PIC14E_TMR0], 0xFF, "model off: TMR0 untouched");
    assert_eq!(p.ram()[INTCON] & PIC14_T0IF, 0, "model off: no flag");
}

#[test]
fn pic14e_external_clock_idles() {
    let mut p = Pic14e::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x20; // T0CS: external T0CKI, out of scope
    p.run(8);
    assert_eq!(p.ram()[PIC14E_TMR0], 0, "external mode never ticks");
}

#[test]
fn pic14_tmr0_write_clears_the_prescaler() {
    // NOP; MOVLW 0; MOVWF TMR0; NOPs. At 1:4 the MOVWF drops the banked
    // cycles, so the first tick lands four cycles after the write.
    let mut p = Pic14::new(vec![0x0000, 0x3000, 0x0081, 0x0000, 0x0000, 0x0000]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = 0x01;
    p.run(3);
    assert_eq!(p.ram()[PIC14_TMR0], 0);
    p.run(2);
    assert_eq!(
        p.ram()[PIC14_TMR0],
        0,
        "cleared prescaler must not tick early"
    );
    p.run(1);
    assert_eq!(p.ram()[PIC14_TMR0], 1, "four cycles after the write tick");
}

#[test]
fn pic14e_psa_set_bypasses_the_prescaler() {
    let mut p = Pic14e::new(vec![0u16; 64]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x08; // PSA: prescaler to WDT, TMR0 1:1
    p.run(3);
    assert_eq!(p.ram()[PIC14E_TMR0], 3, "1:1 advances one per cycle");
}

#[test]
fn pic14e_option_instruction_loads_option_reg() {
    // MOVLW 0xA5; OPTION (0x0062): W -> OPTION_REG.
    let mut p = Pic14e::new(vec![0x30A5, 0x0062, 0x0000]);
    p.run(2);
    assert_eq!(p.ram()[PIC14E_OPTION_REG], 0xA5, "OPTION stores W");
}

#[test]
fn pic14e_sources_do_not_cross_trigger() {
    let mut p = Pic14e::new(prog_with_isr());
    p.set_timer0_enabled(true);
    p.ram_mut()[INTCON] = GIE | PIC14_T0IE;
    p.request_interrupt();
    p.run(2);
    assert_ne!(p.pc(), VECTOR, "INT0 must not vector on T0IE");
    assert!(p.interrupt_pending(), "still latched without INTE");
}

#[test]
fn pic14e_tmr0_write_clears_the_prescaler() {
    // NOP; MOVLW 0; MOVWF TMR0 (0x15); NOPs, same shape as the PIC14 case.
    let mut p = Pic14e::new(vec![0x0000, 0x3000, 0x0095, 0x0000, 0x0000, 0x0000]);
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14E_OPTION_REG] = 0x01;
    p.run(3);
    assert_eq!(p.ram()[PIC14E_TMR0], 0);
    p.run(2);
    assert_eq!(
        p.ram()[PIC14E_TMR0],
        0,
        "cleared prescaler must not tick early"
    );
    p.run(1);
    assert_eq!(p.ram()[PIC14E_TMR0], 1, "four cycles after the write tick");
}
