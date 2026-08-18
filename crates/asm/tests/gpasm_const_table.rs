//! Milestone-10 const-table cross-check: our assembler's HEX for the
//! const_table program must match gpasm byte-for-byte, and the program must
//! run correctly in the simulator (out == 0x82 for in == 290, halted).
//!
//! The fixture is the driver's full-pipeline output (clang -> irparse ->
//! wholeprog -> legalize -> callgraph -> alloc -> isel -> banking ->
//! peephole) on `crates/driver/tests/fixtures/const_table.c`. It exercises
//! the milestone-10 chunked const-table surface end to end: the 40-byte
//! `pad` filler keeps the section far enough past main that `table` lands
//! past 0x100 (base 0x100, window 1 — M11 now skips same-page PCLATH
//! restores, so main is smaller), so the readers' `MOVLW HIGH(...); MOVWF
//! PCLATH` sets are load-bearing; the 300-byte table is split into two
//! 256-byte chunks at `table` (0x100) and `table_1` (0x200, LOW == 0 both)
//! with the `__read_table_hi` entry after the table; the caller splits each
//! runtime index into the in-chunk byte (W) and the chunk bit (0x70) and
//! CALLs the right entry. `in` is the i16 global at 0x20 (low byte holds
//! the input); `out` is the global at 0x22. Total program: 562 words
//! (still a single page).
//!
//! gpasm does not know our two assembly directives, so `to_gpasm_src`
//! translates them to source that assembles to the SAME words: `.table`
//! emits nothing (dropped), and `.align N` — which our assembler pads with
//! 0x0000 NOP words — becomes that many explicit `NOP` lines (an ORG jump
//! would leave a gap in gpasm's HEX and break the byte-for-byte compare).
//! The compare itself proves the translation is word-exact.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// gpasm-compatible rendering of our asm: `.table` lines dropped, `.align N`
/// expanded to explicit NOPs (the 0x0000 fill our assembler emits).
fn to_gpasm_src(src: &str) -> String {
    let mut org = 0usize;
    let mut out: Vec<String> = Vec::new();
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty()
            || line.starts_with("list")
            || line.starts_with("radix")
            || line.contains(" equ ")
            || line.ends_with(':')
        {
            out.push(raw.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            out.push(raw.to_string());
            continue;
        }
        if line.starts_with("end") {
            out.push(raw.to_string());
            break;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            let target = (org + n - 1) & !(n - 1);
            while org < target {
                out.push("    NOP".to_string());
                org += 1;
            }
            continue;
        }
        if line.starts_with(".table ") {
            continue;
        }
        out.push(raw.to_string());
        org += 1;
    }
    out.join("\n")
}

#[test]
fn const_table_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/const_table.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/const_table_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("const_table_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", gpasm_asm.to_str().unwrap(), "-o", "const_table_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/const_table_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 290 -> out = 0x82 (hand-computed,
    // see the trace in crates/driver/tests/const_table_e2e.rs)
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(2_000_000);
    assert_eq!(p.ram()[0x22], 0x82, "out == hand-computed 0x82 for in == 290");
    assert!(p.halted());
}
