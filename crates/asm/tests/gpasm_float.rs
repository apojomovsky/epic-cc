//! Milestone-15 soft-float cross-check: our assembler's HEX for the float
//! program must match gpasm byte-for-byte, and the program must run
//! correctly in the simulator (out1 == 0x3F99999A, out2 == 0x41100000,
//! out3 == 0x3EAAAAAB for in == 3.0f, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/float.c`. It
//! exercises the milestone-15 surface end to end: the soft-float runtime
//! routine bodies (__div_f32 via the noinline `half` helper and the RNE
//! 1.0/3.0 case, __add_f32, __mul_f32, __cmp_f32 + the olt icmp tree,
//! __fptosi_f32, __sitofp_f32 with the i16 source ABI sign-extension),
//! and the sret/byval struct machinery with a float member. The program
//! fits in one page (its `PAGE(label)` operands all resolve to page 0) —
//! the translation below is the generic M11 one (org-gap fills / .align
//! NOPs / dropped .table lines are no-ops here, only `PAGE(label)` ->
//! literal fires).
//!
//! `in` is the float global at 0x20; `out1` at 0x24; `out2` at 0x28;
//! `out3` at 0x2C (the same alloc layout the driver used).
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
fn float_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/float.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/float_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("float_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p16f877a",
            gpasm_asm.to_str().unwrap(),
            "-o",
            "float_gpasm.hex",
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/float_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator: in = 3.0f -> out1 = 0x3F99999A (fdiv
    // call 3.0/2.5), out2 = 0x41100000 (the fadd/fmul/fptosi/sitofp chain
    // 9.75 -> 9 -> 9.0), out3 = 0x3EAAAAAB (the RNE 1.0/3.0 through the
    // struct) — hand-computed, see crates/driver/tests/float_e2e.rs and
    // fixtures/float.c
    let mut p = Pic14::new(parse_hex(&ours));
    let val = 3.0f32.to_bits().to_le_bytes(); // in = 3.0 (little-endian f32)
    for i in 0..4 {
        p.ram_mut()[0x20 + i] = val[i];
    }
    p.run(5_000_000);
    let mut got = [0u8; 4];
    for i in 0..4 {
        got[i] = p.ram()[0x24 + i];
    }
    assert_eq!(
        got,
        (3.0f32 / 2.5f32).to_bits().to_le_bytes(),
        "out1 == 1.2 = 0x3F99999A"
    );
    for i in 0..4 {
        got[i] = p.ram()[0x28 + i];
    }
    assert_eq!(
        got,
        (((3.0f32 + 0.25f32) * 3.0f32) as i16 as f32)
            .to_bits()
            .to_le_bytes(),
        "out2 == 9.0 = 0x41100000"
    );
    for i in 0..4 {
        got[i] = p.ram()[0x2C + i];
    }
    assert_eq!(
        got,
        (1.0f32 / 3.0f32).to_bits().to_le_bytes(),
        "out3 == 1.0/3.0 = 0x3EAAAAAB (RNE)"
    );
    assert!(p.halted());
}
