//! Milestone-9 multi-bank FSR cross-check: our assembler's HEX for the
//! banked_ptr program must match gpasm byte-for-byte, and the program must
//! run correctly in the simulator (out == 0xB8 for in == 3, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/banked_ptr.c`. It
//! exercises the milestone-9 multi-bank indirect surface end to end: FSR
//! setups that set IRP (BSF STATUS, 7) for the bank-2 (0x120) and bank-3
//! (0x1A0) arrays and clear it (BCF STATUS, 7) for the bank-1 (0xA0) one,
//! including the `& 0xFF` low-byte base literals (0x20 for 0x120), plus a
//! banked direct copy (BANKSELs), an sret call into a bank-3 frame alloca
//! (IRP dance from the stored hi byte in mk), and a chained dynamic index.
//! `in` is the i16 global at 0x20 (low byte holds the input); `out` is the
//! global at 0x1B0 (bank 3).
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn banked_ptr_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/banked_ptr.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/banked_ptr_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p16f877a",
            "banked_ptr.asm",
            "-o",
            "banked_ptr_gpasm.hex",
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/banked_ptr_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 3 -> out = 0xB8 (hand-computed,
    // see the trace in crates/driver/tests/banked_ptr_e2e.rs)
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 3; // in low byte = 3 (high byte stays 0)
    p.run(2_000_000);
    assert_eq!(
        p.ram()[0x1B0],
        0xB8,
        "out == hand-computed 0xB8 for in == 3"
    );
    assert!(p.halted());
}
