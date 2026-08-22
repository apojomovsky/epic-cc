use pic14_sim::Pic14;

// Main occupies words 0-3; the F877A interrupt vector is word 4, where the
// ISR begins. fire_interrupt is called BETWEEN steps, so `pc` addresses an
// instruction that has not executed yet: it pushes that pc, and RETFIE
// (0x0009) pops it so the interrupted instruction still runs.
//
// The busy-loop is what makes the distinction visible. Main spins at word 2
// (`GOTO 0x02`), so resuming at word 2 keeps it spinning, which is what an
// interrupt must do. Pushing pc + 1 would resume at word 3 and walk the
// program out of its own loop.
//
// Program (hand-encoded 14-bit words):
//   0: MOVLW 0x05     0x3005   ; W = 5
//   1: MOVWF 0x20     0x00A0   ; RAM[0x20] = 5
//   2: GOTO 0x02      0x2802   ; main busy-loop (fires here: pc=2)
//   3: NOP            0x0000   ; never reached: the loop at word 2 is closed
//   4: MOVWF 0x75     0x00F5   ; ISR: save W -> RAM[0x75] = 5
//   5: MOVLW 0xAA     0x30AA   ; W = 0xAA
//   6: MOVWF 0x76     0x00F6   ; ISR side-effect -> RAM[0x76] = 0xAA
//   7: RETFIE         0x0009   ; pop return -> pc = 2
fn interrupt_program() -> Vec<u16> {
    vec![
        0x3005, 0x00A0, 0x2802, 0x0000, 0x00F5, 0x30AA, 0x00F6, 0x0009,
    ]
}

#[test]
fn fire_interrupt_jumps_to_vector_and_retfie_resumes() {
    let mut p = Pic14::new(interrupt_program());

    // Run the main busy-loop a few steps: W=5, RAM[0x20]=5, then spin at word 2.
    p.run(10);
    assert_eq!(p.pc(), 2);
    assert_eq!(p.w(), 5);
    assert_eq!(p.ram()[0x20], 5);

    // Inject the interrupt: the return address (pc = 2, the instruction that
    // has not run yet) is pushed, PC -> vector 4.
    p.fire_interrupt();
    assert_eq!(p.pc(), 4);

    // The ISR runs from word 4...
    p.step(); // MOVWF 0x75 (save W)
    assert_eq!(p.ram()[0x75], 5);
    p.step(); // MOVLW 0xAA
    assert_eq!(p.w(), 0xAA);
    p.step(); // MOVWF 0x76
    assert_eq!(p.ram()[0x76], 0xAA);

    // ...and RETFIE pops the pushed return: back to word 2, the instruction
    // the interrupt preempted, so main resumes its loop instead of falling
    // out of it.
    p.step(); // RETFIE
    assert_eq!(
        p.pc(),
        2,
        "main resumes at the preempted instruction, not past it"
    );
    p.run(5);
    assert_eq!(p.pc(), 2, "and it is still spinning in its loop");
}
