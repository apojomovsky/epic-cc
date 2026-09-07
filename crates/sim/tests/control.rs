//! Phase-3 debugger control surface over `Pic14` (epic-cc#258):
//! `run_until` stop reasons, register round-trips, banked memory
//! read/write across a bank boundary, program-flash reads, and the
//! determinism guarantee (same state + same arguments -> byte-identical
//! RAM and the same stop reason).

use pic14_sim::{Pic14, StopReason};

//   0: MOVLW 0x11     0x3011   ; W = 0x11
//   1: MOVWF 0x20     0x00A0   ; RAM[bank0 0x20] = W
//   2: GOTO 0x02      0x2802   ; spin here forever
fn spin_program() -> Vec<u16> {
    vec![0x3011, 0x00A0, 0x2802]
}

//   0: MOVLW 0x55     0x3055
//   1: MOVWF 0x70     0x00F0   ; common RAM, visible from every bank
//   2: NOP            0x0000
fn short_program() -> Vec<u16> {
    vec![0x3055, 0x00F0, 0x0000]
}

#[test]
fn run_until_hits_the_target_exactly() {
    let mut p = Pic14::new(short_program());
    // The stop is breakpoint semantics: pc lands on the target after
    // earlier words executed, before the target word itself has.
    assert_eq!(p.run_until(1, 10), StopReason::Reached);
    assert_eq!(p.pc(), 1);
    assert_eq!(p.w(), 0x55, "word 0 executed, word 1 has not");
    p.step();
    assert_eq!(p.ram()[0x70], 0x55, "resuming runs the target word");
}

#[test]
fn run_until_reports_halted_when_the_target_never_hits() {
    let mut p = Pic14::new(short_program());
    // The program runs off its end at word 3 without ever reaching 0xFF.
    assert_eq!(p.run_until(0xFF, 100), StopReason::Halted);
    assert!(p.halted());
}

#[test]
fn run_until_stops_at_the_cap_with_the_right_reason() {
    let mut p = Pic14::new(spin_program());
    // The spin loop never reaches 0xFF, so only the cap can stop it,
    // inside the loop at word 2.
    assert_eq!(p.run_until(0xFF, 7), StopReason::Capped);
    assert_eq!(p.pc(), 2, "stopped inside the spin loop");
}

#[test]
fn run_until_is_reached_immediately_from_the_target() {
    let mut p = Pic14::new(short_program());
    p.set_pc(2);
    assert_eq!(p.run_until(2, 0), StopReason::Reached);
    assert_eq!(p.pc(), 2);
}

#[test]
fn register_writes_round_trip() {
    let mut p = Pic14::new(spin_program());
    // Bank select bits: a written bank is what the machine pages by.
    p.set_bank(2);
    assert_eq!(p.bank(), 2);
    assert_eq!(p.status() >> 5 & 0b11, 2, "RP1:RP0 in STATUS");
    // A PC write resumes from the written address.
    p.set_pc(1);
    p.step();
    assert_eq!(p.pc(), 2);
    // Resuming at word 1 skips word 0, so W is still 0 and MOVWF
    // stores that: execution demonstrably resumed mid-program.
    assert_eq!(p.w(), 0);
    assert_eq!(p.ram()[0x20], 0);
    p.set_w(0x42);
    assert_eq!(p.w(), 0x42, "W write round-trips");
    // Register reads expose the SFR image.
    assert_eq!(p.fsr(), 0);
    assert_eq!(p.pclath(), 0);
    assert_eq!(p.intcon(), 0);
}

#[test]
fn memory_round_trips_across_a_bank_boundary() {
    let mut p = Pic14::new(short_program());
    // Bank 1: direct operand 0x20 resolves to physical 0xA0.
    p.set_bank(1);
    p.write_mem(0x20, &[1, 2, 3, 4]);
    assert_eq!(p.ram()[0xA0], 1);
    assert_eq!(p.ram()[0xA3], 4);
    assert_eq!(p.ram()[0x20], 0, "the bank-0 alias stays untouched");
    assert_eq!(p.read_mem(0x20, 4), vec![1, 2, 3, 4]);
    // Back in bank 0 the same direct operand reads bank 0's image.
    p.set_bank(0);
    assert_eq!(p.read_mem(0x20, 1), vec![0]);
    p.write_mem(0x20, &[0x99]);
    assert_eq!(p.ram()[0x20], 0x99);
    assert_eq!(p.ram()[0xA0], 1, "bank 1's byte is untouched");
}

#[test]
#[should_panic(expected = "outside the 7-bit direct-operand space")]
fn memory_rejects_addresses_no_operand_can_name() {
    let mut p = Pic14::new(short_program());
    // 0x80 is not a direct operand: no ISA operand can name it, so the
    // surface refuses it instead of silently touching another bank.
    p.write_mem(0x80, &[0x77]);
}

#[test]
fn program_flash_reads_return_the_word() {
    let p = Pic14::new(short_program());
    assert_eq!(p.read_prog_word(0), Some(0x3055));
    assert_eq!(p.read_prog_word(2), Some(0x0000));
    assert_eq!(p.read_prog_word(3), None, "past the program's end");
}

#[test]
fn run_until_is_deterministic() {
    let run = || {
        let mut p = Pic14::new(spin_program());
        let reason = p.run_until(1, 5);
        (reason, p.w(), *p.ram(), p.pc())
    };
    assert_eq!(run(), run());
}
