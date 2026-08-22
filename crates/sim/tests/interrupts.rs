//! Interrupt semantics: the return address, and GIE / enable-bit gating.
//!
//! `fire_interrupt` is the unconditional test hook — it vectors regardless of
//! INTCON, which is what the milestone-13 e2e tests drive. `request_interrupt`
//! is the modelled path: the request latches, sets INTF, and is taken only
//! once GIE and INTE are both set, so a program that masks interrupts can be
//! verified.
use pic14_sim::Pic14;

const INTCON: usize = 0x0B;
const GIE: u8 = 1 << 7;
const INTE: u8 = 1 << 4;
const INTF: u8 = 1 << 1;

/// word 0: MOVLW 0x01, word 1: MOVWF 0x20, word 2: SLEEP,
/// word 3: NOP, word 4 (the vector): RETFIE.
fn prog_store_then_halt() -> Vec<u16> {
    vec![0x3001, 0x00A0, 0x0063, 0x0000, 0x0009]
}

#[test]
fn the_instruction_at_the_injection_point_is_not_skipped() {
    // fire_interrupt is called BETWEEN steps, so `pc` addresses an
    // instruction that has not run yet. The pushed return address must be
    // that pc — pushing pc + 1 silently drops the instruction.
    let mut p = Pic14::new(prog_store_then_halt());
    p.step(); // MOVLW 0x01; pc is now 1, at the MOVWF
    assert_eq!(p.pc(), 1);
    p.fire_interrupt();
    assert_eq!(p.pc(), 4, "the vector is word 4");
    p.run(100);
    assert!(p.halted());
    assert_eq!(
        p.ram()[0x20],
        0x01,
        "the MOVWF at the injection pc must still run"
    );
}

#[test]
fn a_masked_request_stays_pending_until_gie_is_set() {
    // GIE clear: the request latches instead of vectoring.
    let mut p = Pic14::new(prog_store_then_halt());
    p.ram_mut()[INTCON] = INTE; // enabled source, but GIE clear
    p.step();
    let pc_before = p.pc();
    p.request_interrupt();
    assert_eq!(p.pc(), pc_before, "a masked request must not vector");
    assert_ne!(
        p.ram()[INTCON] & INTF,
        0,
        "the request sets the source flag"
    );
    assert!(
        p.interrupt_pending(),
        "the request stays pending while masked"
    );

    // Unmasking takes it at the next step boundary.
    p.ram_mut()[INTCON] |= GIE;
    p.step();
    assert_eq!(p.pc(), 4, "unmasking GIE takes the pending interrupt");
    assert!(!p.interrupt_pending(), "the latch is consumed when taken");
}

#[test]
fn a_request_with_the_source_disabled_stays_pending() {
    // GIE set but INTE clear: still masked.
    let mut p = Pic14::new(prog_store_then_halt());
    p.ram_mut()[INTCON] = GIE;
    p.step();
    let pc_before = p.pc();
    p.request_interrupt();
    p.step();
    assert_ne!(
        p.pc(),
        4,
        "the source is disabled, so the interrupt is not taken"
    );
    assert!(p.interrupt_pending(), "it stays pending");
    assert!(p.pc() > pc_before, "and the program keeps running");
}

#[test]
fn an_enabled_request_is_taken_and_clears_gie() {
    let mut p = Pic14::new(prog_store_then_halt());
    p.ram_mut()[INTCON] = GIE | INTE;
    p.step();
    p.request_interrupt();
    p.step();
    assert_eq!(p.pc(), 4, "an enabled request vectors at the next step");
    assert_eq!(
        p.ram()[INTCON] & GIE,
        0,
        "hardware clears GIE on entry so the handler is not re-entered"
    );
}

#[test]
fn retfie_restores_gie() {
    let mut p = Pic14::new(prog_store_then_halt());
    p.ram_mut()[INTCON] = GIE | INTE;
    p.step();
    p.request_interrupt();
    p.step(); // vector
    assert_eq!(p.ram()[INTCON] & GIE, 0, "GIE is clear inside the handler");
    p.step(); // RETFIE
    assert_ne!(p.ram()[INTCON] & GIE, 0, "RETFIE sets GIE again");
}

#[test]
fn a_pending_request_does_not_re_enter_the_handler() {
    // The latch is consumed on entry, so the handler runs once even though
    // RETFIE re-enables GIE and the program never clears INTF.
    let mut p = Pic14::new(prog_store_then_halt());
    p.ram_mut()[INTCON] = GIE | INTE;
    p.request_interrupt();
    p.run(200);
    assert!(
        p.halted(),
        "the program must reach SLEEP, not loop in the handler"
    );
    assert_eq!(p.ram()[0x20], 0x01, "and it still does its work");
}
