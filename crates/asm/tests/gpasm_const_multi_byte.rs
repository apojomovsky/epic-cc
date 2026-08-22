//! Issue #3 cross-check: our assembler's HEX for the multi-byte const-table
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (the eight hand-computed outputs for in ==
//! 290, halted).
//!
//! The fixture is the driver's full-pipeline output (clang -> irparse ->
//! wholeprog -> legalize -> callgraph -> alloc -> isel -> banking ->
//! peephole) on `crates/driver/tests/fixtures/const_multi_byte.c`. It
//! exercises the issue-#3 surface end to end: const tables of i16 / i32 /
//! float elements (typed element-list initializers decoded little-endian
//! by irparse), read at runtime through the scale-2/scale-4 index
//! sequences (RLF pairs — classic mid-range has no MULLW) into both the
//! small RETLW tables (`t16s`, `t32s`) and the chunked large-table
//! readers (`t16` = 260 bytes, `t32` = 400, `tf` = 400 — chunk 1 selected
//! by the scale's carry). The program is a single page, so the readers'
//! `MOVLW PAGE(...); MOVWF PCLATH` sets are the only page tokens.
//!
//! gpasm does not know our directives or `PAGE(label)`, so `to_gpasm_src`
//! translates them to source that assembles to the SAME words: `.table`
//! lines dropped, `.align N` padded with explicit NOPs (the 0x0000 fill
//! our assembler emits), org gaps filled with `fill 0x0000`, and every
//! `PAGE(label)` operand resolved to its literal `(addr >> 11) << 3`. The
//! byte-for-byte compare proves the translation is word-exact.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::collections::HashMap;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

fn label_addrs(src: &str) -> HashMap<String, usize> {
    let mut sym = HashMap::new();
    let mut org = 0usize;
    for raw in src.lines() {
        let t = raw.split(';').next().unwrap_or("").trim();
        if let Some(rest) = t.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
        } else if let Some(n) = t.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
        } else if let Some(l) = t.strip_suffix(':') {
            sym.insert(l.trim().to_string(), org);
        } else if t.starts_with(".table ") || t.starts_with("end") || t.is_empty() {
            // no word
        } else {
            org += 1;
        }
    }
    sym
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
                rendered = format!(
                    "{}{}0x{lit:02X}{}",
                    &rendered[..start],
                    " ",
                    &rendered[start + 5 + end + 1..]
                );
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
fn const_multi_byte_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/const_multi_byte.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/const_multi_byte_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("const_multi_byte_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p16f877a",
            gpasm_asm.to_str().unwrap(),
            "-o",
            "const_multi_byte_gpasm.hex",
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/const_multi_byte_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");

    // and it runs in the simulator: in = 290 -> the hand-computed outputs
    // (see the trace in crates/driver/tests/const_multi_byte_e2e.rs).
    // Global addresses follow the alloc layout: `in` is the i16 at 0x20;
    // out_s16/out_s32 at 0x22-0x25, out_l16/out_l16b at 0x26-0x29,
    // out_l32/out_l32b at 0x2A-0x31, outf/outf2 at 0x32-0x39.
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(5_000_000);
    let r16 = |addr: usize| u16::from_le_bytes([p.ram()[addr], p.ram()[addr + 1]]);
    let r32 = |addr: usize| {
        let mut v = 0u32;
        for i in 0..4 {
            v |= (p.ram()[addr + i] as u32) << (8 * i);
        }
        v
    };
    assert_eq!(r16(0x22), 0x9ABC, "out_s16");
    assert_eq!(r32(0x24), 0x090A0B0C, "out_s32");
    assert_eq!(r16(0x28), 0x1022, "out_l16");
    assert_eq!(r16(0x2A), 0x1080, "out_l16b");
    assert_eq!(r32(0x2C), 0x23242526, "out_l32");
    assert_eq!(r32(0x30), 0x41424344, "out_l32b");
    assert_eq!(r32(0x34), 0x4059999A, "outf");
    assert_eq!(r32(0x38), 0x40CCCCCD, "outf2");
    assert!(p.halted());
}
