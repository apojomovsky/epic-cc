//! Milestone-13 interrupt cross-check: our assembler's HEX for the
//! interrupt program must match gpasm byte-for-byte, and the program must
//! run correctly in the simulator with the interrupt fired mid-run (out ==
//! 0x16 for in == 0x10, PORTB == 0x22, halted).
//!
//! The fixture was captured from the driver's full pipeline (clang ->
//! irparse -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
//! banking -> peephole) on `crates/driver/tests/fixtures/interrupt.c`. It
//! exercises the milestone-13 surface end to end: the `.org 4` vector entry
//! (the ISR's code starts AT word 4 — no GOTO), the save prologue
//! (`MOVWF 0x75` / `SWAPF 0x75, F` / `SWAPF STATUS, W` / `MOVWF 0x76` /
//! PCLATH/FSR -> 0x77/0x78 / retval 0x71-0x74 -> 0x79-0x7C / scratch
//! 0x70 -> 0x7D / PCLATH = 0), the ISR body (a direct SFR store
//! `MOVWF 0x06` from the `inttoptr` literal pointer, a call to the
//! duplicated shared helper `bump_isr`), the restore epilogue (retval
//! first, then the scratch 0x7D -> 0x70, PCLATH/FSR, `SWAPF 0x76, W` back
//! into STATUS, W last via `SWAPF 0x75, W` — all flag-safe) and `RETFIE`
//! — the SWAPF/RETFIE encodings gpasm verifies.
//!
//! `in` is the volatile i8 global at 0x21, `out` at 0x20; PORTB is the
//! F877A SFR at RAM[0x06] (the same alloc layout the driver used).
//!
//! The sim run drives the same scenario as the e2e (crates/driver/tests/
//! interrupt_e2e.rs): fire the interrupt at main's word 79 (the `%2 = load
//! out` for `out = bump(out)`, right after the `PORTB = 0x11` store), so
//! the ISR bumps out 0x10 -> 0x11 before main's bump reads it; main
//! completes 0x11 -> 0x12 -> 0x13 -> 0x16 and PORTB = 0x22, then the
//! __start SLEEP halts the machine.
use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::collections::HashMap;
use std::process::Command;

/// The interrupt vector (word 4) and the injection point (word 79) as
/// documented in crates/driver/tests/interrupt_e2e.rs.
const VECTOR: u16 = 4;
const INJECT_PC: u16 = 79;

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
fn interrupt_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/interrupt.asm");
    let ours = assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/interrupt_ours.hex"), &ours).unwrap();
    let gpasm_src = to_gpasm_src(src);
    let gpasm_asm = std::env::temp_dir().join("interrupt_gpasm.asm");
    std::fs::write(&gpasm_asm, &gpasm_src).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", gpasm_asm.to_str().unwrap(), "-o", "interrupt_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/interrupt_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator exactly like the e2e: in = 0x10, fire at
    // word 79 -> out = 0x16, PORTB = 0x22, halted.
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x21] = 0x10; // in
    let mut steps = 0usize;
    while p.pc() != INJECT_PC {
        p.step();
        steps += 1;
        assert!(steps < 200, "never reached the injection point (pc = {})", p.pc());
    }
    assert_eq!(p.ram()[0x20], 0x10, "out == in before the ISR");
    assert_eq!(p.ram()[0x06], 0x11, "PORTB == 0x11 (main's SFR write) before the ISR");
    p.fire_interrupt();
    assert_eq!(p.pc(), VECTOR, "the ISR starts at the vector (word 4)");
    p.run(500_000);
    assert_eq!(p.ram()[0x20], 0x16, "out == hand-computed 0x16 (ISR bump 0x10 -> 0x11, then 0x11 -> 0x12 -> 0x13 -> 0x16)");
    assert_eq!(p.ram()[0x06], 0x22, "PORTB == 0x22 (main's final SFR write)");
    assert!(p.halted());
}
