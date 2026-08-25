//! epic-cc#117 cross-check: our assembler's HEX for the runtime-SFR-address
//! program must match gpasm byte-for-byte, and the program must run correctly
//! in the simulator.
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/runtime_sfr.c`. It
//! exercises all three runtime-address shapes: the standalone `inttoptr`
//! (`MOVWF FSR; MOVF INDF, W`), the pointer select over literal inttoptrs
//! (the two-byte select materialization `MOVLW 0x0C/0x0D` into the address
//! slot), and the pointer phi join (phi copies moving the address bytes,
//! then `BTFSC slot+1, 0` setting IRP before the `INDF` access). The
//! indirect accesses carry no BANKSEL by construction: INDF reaches the
//! whole linear file space through FSR+IRP.
//!
//! The fixture carries the M10 `.table` window-fit directive (which gpasm
//! does not know), so it goes through the same `to_gpasm_src` translation
//! as the other const-table cross-checks.
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
fn runtime_sfr_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/runtime_sfr.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/runtime_sfr_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("runtime_sfr_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p16f877a",
            gpasm_asm.to_str().unwrap(),
            "-o",
            "runtime_sfr_gpasm.hex",
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/runtime_sfr_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");

    // And it runs: preload PIR2 (0x0D) with BCLIF (0x01), irq = 4 selects
    // the PIR2 arm, GetFlag returns 1, ClearFlag clears the bit, and the
    // standalone write_offset(0, 0xAA) writes PIR1. Hand-derived from the
    // driver e2e: out_flag = 1 + 0 + read_offset(0)=0x00 + read_offset(1)=
    // 0x00 = 1; out_clear = PIR1|PIR2 = 0x00; out_write = 0xAA.
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 4; // irq (layout from the driver run)
    p.ram_mut()[0x0B] = 0x00; // INTCON
    p.ram_mut()[0x0C] = 0x00; // PIR1
    p.ram_mut()[0x0D] = 0x01; // PIR2: BCLIF set
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[0x21], 1, "out_flag irq=4");
    assert_eq!(p.ram()[0x22], 0x00, "out_clear irq=4");
    assert_eq!(p.ram()[0x23], 0xAA, "out_write irq=4");
}
