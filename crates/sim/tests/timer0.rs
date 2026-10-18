//! Timer0 on PIC14, PIC14E and PIC18: prescaled TMR0 advance, overflow
//! flag, and the TMR0IE vector path. The model is opt-in
//! (`set_timer0_enabled`), internal clock only, with the rate from live
//! OPTION_REG (PIC14/E) or T0CON (PIC18) state.
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

mod pic18 {
    use pic14_sim::{
        Pic18, PIC18_GIEH, PIC18_HI_VECTOR, PIC18_INT0IE, PIC18_INTCON, PIC18_T0CON, PIC18_TMR0H,
        PIC18_TMR0IE, PIC18_TMR0IF, PIC18_TMR0L,
    };

    const RETFIE: u16 = 0x0010;
    /// TMR0ON, 8-bit, internal clock, prescaler 1:4.
    const T0CON_8BIT_1_4: u8 = 0xC1;
    /// MOVWF TMR0L,A: 0xFD6 is access-bank offset 0xD6.
    const MOVWF_TMR0L: u16 = 0x6ED6;

    /// Enough NOPs for the longest run below.
    fn nops() -> Vec<u16> {
        vec![0u16; 512]
    }

    fn armed(t0con: u8) -> Pic18 {
        let mut p = Pic18::new(nops());
        p.set_timer0_enabled(true);
        p.ram_mut()[PIC18_T0CON] = t0con;
        p
    }

    #[test]
    fn advances_by_cycles_over_prescaler() {
        let mut p = armed(T0CON_8BIT_1_4);
        p.ram_mut()[PIC18_TMR0L] = 0xFF;
        p.run(3);
        assert_eq!(p.ram()[PIC18_TMR0L], 0xFF, "3 cycles at 1:4 is no tick");
        assert_eq!(p.ram()[PIC18_INTCON] & PIC18_TMR0IF, 0);
        p.run(1);
        assert_eq!(p.ram()[PIC18_TMR0L], 0x00, "4 cycles tick once and wrap");
        assert_ne!(p.ram()[PIC18_INTCON] & PIC18_TMR0IF, 0, "wrap sets TMR0IF");
        assert_eq!(p.ram()[PIC18_TMR0H], 0, "8-bit mode leaves TMR0H alone");
    }

    #[test]
    fn overflow_vectors_on_gieh_and_tmr0ie() {
        // The high vector (byte 8) is also where 4 NOPs fall through, so
        // the vector shows in the cycle count: 4 + 2, not 4 + 1.
        let mut prog = nops();
        prog[4] = RETFIE;
        let mut p = Pic18::new(prog);
        p.set_timer0_enabled(true);
        p.ram_mut()[PIC18_T0CON] = T0CON_8BIT_1_4;
        p.ram_mut()[PIC18_TMR0L] = 0xFF;
        p.ram_mut()[PIC18_INTCON] = PIC18_GIEH | PIC18_TMR0IE;
        p.run(4);
        assert_eq!(p.cycles(), 4);
        p.step();
        assert_eq!(p.pc(), PIC18_HI_VECTOR, "high vector taken");
        assert_eq!(p.cycles(), 6, "vector entry costs 2 cycles");
        assert_eq!(
            p.ram()[PIC18_INTCON] & PIC18_GIEH,
            0,
            "GIEH cleared on entry"
        );
    }

    #[test]
    fn int0ie_alone_does_not_vector_on_overflow() {
        let mut p = armed(T0CON_8BIT_1_4);
        p.ram_mut()[PIC18_TMR0L] = 0xFF;
        p.ram_mut()[PIC18_INTCON] = PIC18_GIEH | PIC18_INT0IE;
        p.run(5);
        assert_eq!(p.pc(), 10, "5 NOPs, no vector entry");
        assert_eq!(p.cycles(), 5);
        assert_ne!(p.ram()[PIC18_INTCON] & PIC18_TMR0IF, 0);
    }

    #[test]
    fn sixteen_bit_carries_into_tmr0h() {
        let mut p = armed(0x81); // TMR0ON, 16-bit, internal, 1:4
        p.ram_mut()[PIC18_TMR0L] = 0xFF;
        p.run(4);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
        assert_eq!(p.ram()[PIC18_TMR0H], 1, "low byte carry ticks TMR0H");
        assert_eq!(
            p.ram()[PIC18_INTCON] & PIC18_TMR0IF,
            0,
            "no 16-bit wrap yet"
        );
        p.ram_mut()[PIC18_TMR0L] = 0xFF;
        p.ram_mut()[PIC18_TMR0H] = 0xFF;
        p.run(4);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
        assert_eq!(p.ram()[PIC18_TMR0H], 0);
        assert_ne!(
            p.ram()[PIC18_INTCON] & PIC18_TMR0IF,
            0,
            "0xFFFF wrap sets TMR0IF"
        );
    }

    #[test]
    fn psa_set_bypasses_the_prescaler() {
        let mut p = armed(0xC8); // TMR0ON, 8-bit, internal, PSA
        p.run(5);
        assert_eq!(p.ram()[PIC18_TMR0L], 5, "one tick per cycle");
    }

    #[test]
    fn prescaler_select_scales_the_rate() {
        let mut p = armed(0xC7); // 1:256
        p.run(255);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
        p.run(1);
        assert_eq!(p.ram()[PIC18_TMR0L], 1);
    }

    #[test]
    fn timer_stays_off_unless_enabled() {
        let mut p = Pic18::new(nops());
        p.ram_mut()[PIC18_T0CON] = T0CON_8BIT_1_4;
        p.run(8);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
    }

    #[test]
    fn tmr0on_clear_stops_the_timer() {
        let mut p = armed(T0CON_8BIT_1_4 & !0x80);
        p.run(8);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
    }

    #[test]
    fn external_clock_idles() {
        // The POR value of T0CON is 0xFF: on, external clock.
        let mut p = Pic18::new(nops());
        p.set_timer0_enabled(true);
        p.run(8);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
        p.ram_mut()[PIC18_T0CON] = T0CON_8BIT_1_4 | 0x20;
        p.run(8);
        assert_eq!(p.ram()[PIC18_TMR0L], 0, "T0CS set stays idle");
    }

    #[test]
    fn tmr0l_write_clears_the_prescaler() {
        // NOP; NOP; MOVWF TMR0L; NOPs. At 1:4 the MOVWF drops the banked
        // cycles, so the first tick lands four cycles after the write.
        let mut prog = vec![0u16; 16];
        prog[2] = MOVWF_TMR0L;
        let mut p = Pic18::new(prog);
        p.set_timer0_enabled(true);
        p.ram_mut()[PIC18_T0CON] = T0CON_8BIT_1_4;
        p.run(3);
        assert_eq!(p.ram()[PIC18_TMR0L], 0);
        p.run(2);
        assert_eq!(
            p.ram()[PIC18_TMR0L],
            0,
            "cleared prescaler must not tick early"
        );
        p.run(1);
        assert_eq!(p.ram()[PIC18_TMR0L], 1, "four cycles after the write tick");
    }
}
