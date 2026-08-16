use pic14_sim::Pic14;

// Main occupies words 0-3; the F877A interrupt vector is word 4, where the
// ISR begins. fire_interrupt pushes pc+1 (the return address) and jumps to 4;
// RETFIE (0x0009) pops the return so the interrupted main resumes at the exact
// next instruction.
//
// Program (hand-encoded 14-bit words):
//   0: MOVLW 0x05     0x3005   ; W = 5
//   1: MOVWF 0x20     0x00A0   ; RAM[0x20] = 5
//   2: GOTO 0x02      0x2802   ; main busy-loop (fires here: pc=2)
//   3: NOP            0x0000   ; the resume point (pc+1 = 3)
//   4: MOVWF 0x75     0x00F5   ; ISR: save W -> RAM[0x75] = 5
//   5: MOVLW 0xAA     0x30AA   ; W = 0xAA
//   6: MOVWF 0x76     0x00F6   ; ISR side-effect -> RAM[0x76] = 0xAA
//   7: RETFIE         0x0009   ; pop return -> pc = 3
fn interrupt_program() -> Vec<u16> {
    vec![0x3005, 0x00A0, 0x2802, 0x0000, 0x00F5, 0x30AA, 0x00F6, 0x0009]
}

#[test]
fn fire_interrupt_jumps_to_vector_and_retfie_resumes() {
    let mut p = Pic14::new(interrupt_program());

    // Run the main busy-loop a few steps: W=5, RAM[0x20]=5, then spin at word 2.
    p.run(10);
    assert_eq!(p.pc(), 2);
    assert_eq!(p.w(), 5);
    assert_eq!(p.ram()[0x20], 5);

    // Inject the interrupt: return address (pc+1=3) pushed, PC -> vector 4.
    p.fire_interrupt();
    assert_eq!(p.pc(), 4);

    // The ISR runs from word 4...
    p.step(); // MOVWF 0x75 (save W)
    assert_eq!(p.ram()[0x75], 5);
    p.step(); // MOVLW 0xAA
    assert_eq!(p.w(), 0xAA);
    p.step(); // MOVWF 0x76
    assert_eq!(p.ram()[0x76], 0xAA);

    // ...and RETFIE pops the pushed return: back to word 3 (pc+1 at fire time),
    // i.e. the interrupted code resumes at the exact next instruction.
    p.step(); // RETFIE
    assert_eq!(p.pc(), 3);
}
