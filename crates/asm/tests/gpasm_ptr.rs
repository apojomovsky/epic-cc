//! Milestone-5 pointer/const cross-check: our assembler's HEX for the
//! ptr_probe program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out == 20 for in == 1, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/ptr_probe.c`. It
//! exercises both milestone-5 lowerings end to end: the FSR/INDF RAM
//! indirect path (`ADDLW 0x22; MOVWF FSR; MOVF INDF, W` / `MOVWF INDF`) and
//! the RETLW const-table reader (`CALL __read_table` -> `ADDLW LOW(table);
//! MOVWF PCL` -> four `RETLW`s). `in` is the 16-bit global at 0x20-0x21;
//! `out` is physical 0x2A; the const `table` lives in flash.
//!
//! The fixture is the driver's full-pipeline output, so it carries the M10
//! `.table` window-fit directive (which gpasm does not know) — the same
//! `to_gpasm_src` translation as the M10 const_table cross-check.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// gpasm-compatible rendering of our asm: `.table` lines dropped (they emit
/// no words; the assembler's window-fit assert is what matters).
fn to_gpasm_src(src: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.starts_with(".table ") {
            continue;
        }
        out.push(raw.to_string());
    }
    out.join("\n")
}

#[test]
fn ptr_probe_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/ptr_probe.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/ptr_probe_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("ptr_probe_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", gpasm_asm.to_str().unwrap(), "-o", "ptr_probe_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/ptr_probe_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 1 -> ram[1] = table[1] = 20 -> out
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 1; // in low byte = 1 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[0x2A], 20, "out == table[1] == 20");
    assert!(p.halted());
}
