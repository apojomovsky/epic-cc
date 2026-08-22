//! P4 const cross-checks: our assembler's HEX for the PIC18 const-table
//! and ptr-probe programs must match gpasm byte-for-byte.
//!
//! Both fixtures were captured from the full PIC18 pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel-pic18 ->
//! asm) on the byte-identical PIC14 fixtures (`const_table.c`,
//! `ptr_probe.c` in `crates/driver/tests/fixtures/`). They exercise the
//! whole P4 surface end to end: TBLRD const reads with static and dynamic
//! (register-indexed) table offsets, the byte-packed `DB` table emission
//! (the 300-byte `const_table` exercises DB packing past a single word),
//! and the RAM-pointer + const read combination in `ptr_probe.c`.
use asm::assemble_file_to_hex;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

fn gpasm_cross_check(fixture: &str, src: &str) {
    let ours = assemble_file_to_hex(&device::PIC18F4550, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/{fixture}_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p18f4550",
            &format!("{fixture}.asm"),
            "-o",
            &format!("{fixture}_gpasm.hex"),
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/{fixture}_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
}

#[test]
fn pic18_const_table_hex_matches_gpasm() {
    let src = include_str!("fixtures/pic18_const_table.asm");
    gpasm_cross_check("pic18_const_table", src);
}

#[test]
fn pic18_ptr_probe_hex_matches_gpasm() {
    let src = include_str!("fixtures/pic18_ptr_probe.asm");
    gpasm_cross_check("pic18_ptr_probe", src);
}
