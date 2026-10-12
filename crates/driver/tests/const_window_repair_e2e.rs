//! epic-cc#879 regression: a const table at a window edge plus simulated
//! drift. The text below is a post-peephole program whose 35-byte table
//! base sits at LOW 0xEA, the shape the driver's schedule/banking drift
//! leaves behind when isel folded the window at its pre-schedule section
//! start (encoder-sim `.str.5` at `0x1CF0`). Without the repair the
//! assembler's window assert panics; with it the table pads to the next
//! boundary and the indexed read returns the right byte in the sim.

/// Post-peephole program with the table drifted across its window: 217
/// dead words park the table base at LOW 0xEA, and the 35-byte table
/// crosses. Word-exact by construction, no layout luck.
fn drifted_text() -> String {
    let mut s = String::from(
        "PCL    equ 0x02\nFSR    equ 0x04\nPCLATH equ 0x0A\n    org 0x0000\n    goto __start\n",
    );
    for _ in 0..217 {
        s.push_str("    MOVLW 0x00\n");
    }
    s.push_str(
        "__start:\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n    CALL main\n    SLEEP\nmain:\n    MOVLW PAGE(__read_t)\n    MOVWF PCLATH\n    MOVLW 0x03\n    CALL __read_t\n    MOVWF 0x20\n    SLEEP\n__read_t:\n    MOVWF 0x70\n    MOVLW HIGH(t)\n    MOVWF PCLATH\n    MOVF 0x70, W\n    ADDLW LOW(t)\n    MOVWF PCL\n    .table t 35\nt:\n",
    );
    for b in 0..35u8 {
        s.push_str(&format!("    RETLW 0x{b:02X}\n"));
    }
    s.push_str("    end\n");
    s
}

#[test]
#[should_panic(expected = "crosses its 256-byte window")]
fn drifted_table_fails_without_repair() {
    // Teeth: the drifted text must die in the assembler exactly the way
    // the encoder-sim build did, so this test cannot pass vacuously.
    asm::assemble(&drifted_text());
}

#[test]
fn repaired_table_pads_and_reads_run() {
    let asm = drifted_text();
    let locs = vec![None; asm.lines().count()];
    let (fixed, _) = driver::const_window::repair_with_locs(&asm, &locs);
    assert_eq!(
        fixed.matches("    .align 256").count(),
        1,
        "exactly one pad:\n{fixed}"
    );
    let prog = asm::assemble(&fixed);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt");
    assert_eq!(p.ram()[0x20], 3, "table byte 3 reads back");
}
