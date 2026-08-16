//! Milestone-12 "long" cross-check: our assembler's HEX for the long
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out == 0x1634943A for in == 0x12345678,
//! sin == -19, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/long.c`. It
//! exercises the whole i32 surface end to end: the runtime routines
//! `__mul_u32`, `__udiv_u32`, `__urem_u32`, `__sdiv_i32`, `__srem_i32`
//! (recipe bodies emitted inline, called via the existing call ABI with the
//! 4-byte param/retval slots), the inline const-count i32 shifts
//! (shl/lshr/ashr), the 4-byte add/or chains, the i32 icmps (ult/ugt with
//! const operands), the trunc/zext/sext widening, and the {i8,i32} struct
//! byval/sret calls (`getb`/`mkp`). The program fits in one page (its
//! `PAGE(label)` operands all resolve to page 0) — the translation below is
//! the generic M11 one (org-gap fills / .align NOPs / dropped .table lines
//! are no-ops here, only `PAGE(label)` -> literal fires).
//!
//! `in` is the i32 global at 0x2A, `out` at 0x2E, `sin` at 0x32 (the same
//! alloc layout the driver used).
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
fn long_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/long.asm");
    let ours = assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/long_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("long_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", gpasm_asm.to_str().unwrap(), "-o", "long_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/long_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 0x12345678, sin = -19 ->
    // out = 0x1634943A (hand-computed, see
    // crates/driver/tests/long_e2e.rs)
    let mut p = Pic14::new(parse_hex(&ours));
    let val: u32 = 0x12345678;
    p.ram_mut()[0x2A] = (val & 0xFF) as u8;
    p.ram_mut()[0x2B] = ((val >> 8) & 0xFF) as u8;
    p.ram_mut()[0x2C] = ((val >> 16) & 0xFF) as u8;
    p.ram_mut()[0x2D] = ((val >> 24) & 0xFF) as u8;
    let sval: u32 = (-19i32) as u32;
    p.ram_mut()[0x32] = (sval & 0xFF) as u8;
    p.ram_mut()[0x33] = ((sval >> 8) & 0xFF) as u8;
    p.ram_mut()[0x34] = ((sval >> 16) & 0xFF) as u8;
    p.ram_mut()[0x35] = ((sval >> 24) & 0xFF) as u8;
    p.run(5_000_000);
    let got = (p.ram()[0x2E] as u32)
        | ((p.ram()[0x2F] as u32) << 8)
        | ((p.ram()[0x30] as u32) << 16)
        | ((p.ram()[0x31] as u32) << 24);
    assert_eq!(got, 0x1634943A, "out == hand-computed 0x1634943A for in == 0x12345678, sin == -19");
    assert!(p.halted());
}
