//! Milestone-6 scalar cross-check: our assembler's HEX for the scalar
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out == 174 for in == 7, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/scalar.c`. It
//! exercises the milestone-6 scalar surface end to end: `sub` (`SUBWF`),
//! `and i8` (`ANDLW`), `or` (`IORLW`), `xor` (`XORLW`/`XORWF`), and the
//! `eq`/`ne`/`ugt`/`ult` icmp predicates (flag materializations feeding
//! `select`s in the loop). `in` is the i8 global at 0x20; `out` is the i8
//! global at 0x21.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn scalar_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/scalar.asm");
    let ours = assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/scalar_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "scalar.asm", "-o", "scalar_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/scalar_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 7 -> out = 174 (hand-computed,
    // see the trace in crates/driver/tests/fixtures/scalar.c)
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 7; // in = 7
    p.run(200_000);
    assert_eq!(p.ram()[0x21], 174, "out == hand-computed 174 for in == 7");
    assert!(p.halted());
}
