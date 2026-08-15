//! Milestone-4 multi-bank cross-check: our assembler's HEX for the banked
//! program must match gpasm byte-for-byte, and the program must run correctly
//! in the bank-aware simulator (out = 255, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/banked.c`. It
//! exercises the milestone-4 banking output end to end: 197 `BCF/BSF STATUS,
//! 5/6` BANKSELs (numeric RP-bit operands — no RP0/RP1 symbol definitions
//! needed) plus bank-relative 7-bit file-register operands. `out` is physical
//! 0xAA (bank 1); sum 1..90 = 4095, low byte 0xFF = 255.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn banked_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/banked.asm");
    let ours = assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/banked_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "banked.asm", "-o", "banked_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/banked_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: out is physical 0xAA (bank 1)
    let mut p = Pic14::new(parse_hex(&ours));
    p.run(2_000_000);
    assert_eq!(p.ram()[0xAA], 255, "sum 1..90 = 4095, low byte 0xFF");
    assert!(p.halted());
}
