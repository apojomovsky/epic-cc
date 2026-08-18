//! Milestone-11 multi-page cross-check: our assembler's HEX for the
//! multi_page program must match gpasm byte-for-byte, and the program must
//! run correctly in the simulator (out == 0xD8 for in == 290, halted).
//!
//! The fixture is the driver's full-pipeline output (clang -> irparse ->
//! wholeprog -> legalize -> callgraph -> alloc -> isel -> banking ->
//! peephole) on `crates/driver/tests/fixtures/multi_page.c` — a 7986-word
//! program whose functions span pages 0-2 (f1 0x0005, F2 0x0800, f2 0x0CFD,
//! F3 0x1000, f3 0x14FD, main 0x158C) with the 300-byte const table and its
//! readers in page 3 (`__read_table` 0x1D46, table 0x1E00, table_1 0x1F00,
//! `__read_table_hi` 0x1F2C). Every CALL is preceded by a
//! `MOVLW PAGE(target); MOVWF PCLATH` set (the pages differ — the sets are
//! load-bearing), and the `.org 0x0800`/`.org 0x1000`/`.org 0x1800` pads
//! pin the page bases.
//!
//! gpasm does not know our `.org`-gap fill semantics (it leaves org gaps
//! unprogrammed), our `.align`/`.table` directives, or `PAGE(label)`
//! operands, so `to_gpasm_src` translates:
//!   - `.org N` with a gap from the running address -> `fill 0x0000, gap`
//!     (gpasm fills with the same 0x0000 words our assembler emits) followed
//!     by the `org N`;
//!   - `.align N` -> that many explicit `NOP` lines (0x0000, our fill);
//!   - `.table ...` -> dropped (emits nothing);
//!   - `PAGE(label)` operands -> the resolved literal `(addr >> 11) << 3`
//!     from a label-address pre-pass (gpasm cannot resolve PAGE itself;
//!     `LOW()`/`HIGH()` it supports natively).
//! The byte-for-byte compare proves the translation is word-exact; the sim
//! run proves the program itself is correct.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::collections::HashMap;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// Pass 1: label -> word address, walking the asm exactly as `asm::assemble`
/// does (org / .align / labels; word lines advance the running address).
fn label_addrs(src: &str) -> HashMap<String, usize> {
    let mut org = 0usize;
    let mut out = HashMap::new();
    for raw in src.lines() {
        let t = raw.split(';').next().unwrap_or("").trim();
        if t.is_empty() || t.starts_with("list") || t.starts_with("radix") || t.contains(" equ ") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
        } else if let Some(n) = t.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
        } else if let Some(l) = t.strip_suffix(':') {
            out.insert(l.trim().to_string(), org);
        } else if t.starts_with(".table ") || t.starts_with("end") {
            // no words
        } else {
            org += 1;
        }
    }
    out
}

/// gpasm-compatible rendering: `.org` gaps become explicit `fill 0x0000`
/// words, `.align N` becomes NOPs, `.table` lines are dropped, and every
/// `PAGE(label)` operand becomes its literal `(addr >> 11) << 3`.
fn to_gpasm_src(src: &str) -> String {
    let sym = label_addrs(src);
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
            let target = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            if target > org {
                // gpasm leaves org gaps unprogrammed (0x3FFF); fill them with
                // the 0x0000 words our assembler emits.
                out.push(format!("    fill 0x0000, 0x{:X}", target - org));
            }
            out.push(raw.to_string());
            org = target;
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
        if line.contains("PAGE(") {
            // Resolve every PAGE(label) to its literal: gpasm does not know
            // the pseudo-function. LOW()/HIGH() are native gpasm.
            let mut rendered = raw.to_string();
            loop {
                let mut it = rendered.match_indices("PAGE(");
                let Some((start, _)) = it.next() else { break };
                let rest = &rendered[start + 5..];
                let Some(end) = rest.find(')') else { break };
                let label = rest[..end].trim();
                let addr = *sym
                    .get(label)
                    .unwrap_or_else(|| panic!("PAGE({label}) label not found"));
                let lit = (addr >> 11) << 3;
                rendered = format!("{}{}0x{lit:02X}{}", &rendered[..start], " ", &rendered[start + 5 + end + 1..]);
            }
            out.push(rendered);
            org += 1;
            continue;
        }
        out.push(raw.to_string());
        org += 1;
    }
    out.join("\n")
}

#[test]
fn multi_page_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/multi_page.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/multi_page_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("multi_page_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", gpasm_asm.to_str().unwrap(), "-o", "multi_page_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/multi_page_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 290 -> out = 0xD8 (hand-computed
    // from the exact emitted IR; see crates/driver/tests/multi_page_e2e.rs)
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(2_000_000);
    assert_eq!(p.ram()[0x22], 0xD8, "out == hand-computed 0xD8 for in == 290");
    assert!(p.halted());
}
