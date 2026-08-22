//! Milestone-8 mul/div/mod/shift cross-check: our assembler's HEX for the
//! muldiv program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out == 210 for in == 301, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/muldiv.c`. It
//! exercises the milestone-8 scalar surface end to end: the runtime
//! routines `__udiv_u16`, `__mul_u16`, `__urem_u16`, `__sdiv_i16`,
//! `__srem_i16`, `__mul_u8`, `__udiv_u8` and the variable-count `__shl_u16`
//! (called via the existing call ABI with the recipe bodies emitted
//! inline), plus inline const `shl`/`lshr`. `in` is the i16 global at
//! 0x20; `out` is the i16 global at 0x22.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn muldiv_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/muldiv.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/muldiv_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "muldiv.asm", "-o", "muldiv_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/muldiv_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 301 -> out = 210 (hand-computed,
    // see the trace in crates/driver/tests/fixtures/muldiv.c and
    // crates/driver/tests/muldiv_e2e.rs)
    let val: u16 = 301; // little-endian i16 at 0x20
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = (val & 0xFF) as u8;
    p.ram_mut()[0x21] = (val >> 8) as u8;
    p.run(500_000);
    let got = (p.ram()[0x22] as u16) | ((p.ram()[0x23] as u16) << 8);
    assert_eq!(got, 210, "out == hand-computed 210 for in == 301");
    assert!(p.halted());
}
