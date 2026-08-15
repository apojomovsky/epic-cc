//! Milestone-3 overlay cross-check: our assembler's HEX for the overlay
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (in = 3 -> out = 84, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/overlay.c`. It
//! exercises the Task-4 banking fix: literal immediates above 0x7F
//! (e.g. `ADDLW 0xFC`, `ADDLW 0xFF`) must encode correctly.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn overlay_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/overlay.asm");
    let ours = assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/overlay_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "overlay.asm", "-o", "overlay_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/overlay_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 3; // in = 3
    p.run(500_000);
    // big_a(3) = 52; big_b(in+1=4) = 32; out = 52 + 32 = 84
    assert_eq!(p.ram()[0x21], 84);
    assert!(p.halted());
}
