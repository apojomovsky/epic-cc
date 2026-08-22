//! Milestone-7 structs cross-check: our assembler's HEX for the structs
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out == 0x4E, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/structs.c`. It
//! exercises the whole M7 surface end to end: the sret call + caller memcpy
//! (`g = mk(3, 0x1234)`, mk writes its sret target through FSR), byval calls
//! (`sum(g)`, `pick(arr)`, caller copies into the byval slots), the dynamic
//! array-in-struct read/store (`arr.v[arr.n]`, `x.v[x.n]` — FSR = base +
//! runtime index with the const array prefix folded in), and the nested
//! struct field math (`go.in.a/b/z` at offsets 0/2/4, i16 adds). `out` is
//! the i8 global at 0x24; all FSR targets stay <= 0xFF.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn structs_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/structs.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/structs_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "structs.asm", "-o", "structs_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/structs_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: no inputs needed — every value is a
    // fixed constant, so out == 0x4E (hand-computed, see the trace in
    // crates/driver/tests/fixtures/structs.c).
    let mut p = Pic14::new(parse_hex(&ours));
    p.run(200_000);
    assert_eq!(p.ram()[0x24], 0x4E, "out == hand-computed 0x4E");
    assert!(p.halted());
}
