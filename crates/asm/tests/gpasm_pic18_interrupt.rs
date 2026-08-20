//! P5 interrupt cross-check: our assembler's HEX for the PIC18 interrupt
//! program must match gpasm byte-for-byte.
//!
//! The fixture was captured from the full PIC18 pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel-pic18 ->
//! asm) on `crates/isel-pic18/tests/fixtures/interrupt_pic18.c`. It
//! exercises the whole P5 surface end to end: the `.org 0x0008` high-vector
//! entry (the ISR's code starts AT the vector — no GOTO indirection), the
//! save prologue (retval snapshot, STATUS/BSR/FSR0L/H/TBLPTRL/H/U via
//! MOVFF, W last), the ISR body (a direct SFR store `MOVWF 0x081,A` from
//! the `inttoptr` literal pointer, a call to the duplicated shared helper
//! `bump_isr`), the restore epilogue (MOVFF reverse order, W last via
//! `MOVF 0x004,W,A`) and `RETFIE`.
//!
//! The `.org 0x0008` leaves the 4 bytes between the reset `goto __start`
//! (ends byte 4) and the vector (byte 8) unfilled; gpasm skips the gap in
//! its HEX, our assembler zero-fills it. The same transformation the PIC14
//! interrupt cross-check uses (`to_gpasm_src` in gpasm_interrupt.rs)
//! renders the gap as explicit `fill 0x0000, 2` words so the two HEX files
//! align byte-for-byte.
use asm::assemble_file_to_hex;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// gpasm-compatible rendering for PIC18: `.org` byte gaps become explicit
/// `fill 0x0000, <bytes>` lines, labels are kept, instructions pass
/// through. No `PAGE()`/`.align`/`.table` handling is needed: PIC18 has no
/// paging, and P5's ISR layout emits no alignment directives. The address
/// walk must match `asm::assemble_pic18`'s pass-1 sizing: 2-word forms
/// (`GOTO`/`CALL`/`LFSR`/`MOVFF`) advance 4 bytes, everything else 2.
fn to_gpasm_src(src: &str) -> String {
    let mut org = 0usize; // byte address
    let mut out: Vec<String> = Vec::new();
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            let target = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            if target > org {
                out.push(format!("    fill 0x0000, {}", target - org));
            }
            out.push(raw.to_string());
            org = target;
            continue;
        }
        if line.starts_with("end") {
            out.push(raw.to_string());
            break;
        }
        if line.ends_with(':') {
            out.push(raw.to_string());
            continue;
        }
        out.push(raw.to_string());
        let mne = line.split_whitespace().next().unwrap_or("");
        let words = if matches!(mne.to_ascii_uppercase().as_str(), "GOTO" | "CALL" | "LFSR" | "MOVFF") {
            2
        } else {
            1
        };
        org += words * 2;
    }
    out.join("\n")
}

#[test]
fn pic18_interrupt_hex_matches_gpasm() {
    let src = include_str!("fixtures/pic18_interrupt_pic18.asm");
    let ours = assemble_file_to_hex(&device::PIC18F4550, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/pic18_interrupt_pic18_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("pic18_interrupt_pic18_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p18f4550", gpasm_asm.to_str().unwrap(), "-o", "/tmp/pic18_interrupt_pic18_gpasm.hex"])
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string("/tmp/pic18_interrupt_pic18_gpasm.hex").unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
}
