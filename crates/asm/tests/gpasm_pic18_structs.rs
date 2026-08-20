//! P3 structs cross-check: our assembler's HEX for the PIC18 structs
//! program must match gpasm byte-for-byte.
//!
//! The fixture was captured from the full PIC18 pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel-pic18 ->
//! asm) on `crates/isel-pic18/tests/fixtures/structs.c` (Task 11's copy of
//! the PIC14 fixture). It exercises the whole P3 surface end to end: the
//! sret call + caller memcpy (`g = mk(3, 0x1234)`, mk writes its sret
//! target through FSR0), byval calls (`sum(g)`, `pick(arr)`, caller copies
//! into the byval slots), the dynamic array-in-struct read/store
//! (`arr.v[arr.n]`, `x.v[x.n]` — FSR0 = base + runtime index), and the
//! nested struct field math (`go.in.a/b/z` at offsets 0/2/4, i16 adds).
use asm::assemble_file_to_hex;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn pic18_structs_hex_matches_gpasm() {
    let src = include_str!("fixtures/pic18_structs.asm");
    let ours = assemble_file_to_hex(&device::PIC18F4550, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/pic18_structs_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p18f4550", "pic18_structs.asm", "-o", "pic18_structs_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/pic18_structs_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
}
