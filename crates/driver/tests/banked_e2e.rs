//! Milestone-4 multi-bank acceptance: 90 volatile i8 globals exceed bank 0's
//! 80-byte GPR region (0x20-0x6F), so accesses to g80..g89 and `out` (bank 1)
//! require BANKSELs. Acceptance: (a) the emitted .asm contains at least one
//! numeric BANKSEL (`BCF/BSF STATUS, 5/6` — the banking pass emits numeric RP
//! bit operands so no RP0/RP1 symbols are needed anywhere), and (b) the
//! driver's HEX runs to halt in the bank-aware simulator with
//! `out == 255` (sum 1..90 = 4095; clang narrows the accumulator to i8 since
//! only the low byte is stored, so `out = 4095 mod 256 = 255`).

use std::collections::HashMap;
use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Run clang + the full IR pipeline on the banked fixture, exactly as the
/// driver does, and return the alloc layout plus the final (banked) .asm.
fn banked_pipeline() -> (alloc::AllocLayout, String) {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/banked.c"),
        &driver::clang::Options::default(),
    );

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let asm = peephole::optimize(&asm);
    (layout, asm)
}

#[test]
fn banked_asm_contains_banksel() {
    let (_, asm) = banked_pipeline();
    // Numeric RP-bit operands: RP0 = STATUS bit 5, RP1 = STATUS bit 6.
    let banksels: Vec<&str> = asm
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("BSF STATUS, 5")
                || t.starts_with("BCF STATUS, 5")
                || t.starts_with("BSF STATUS, 6")
                || t.starts_with("BCF STATUS, 6")
        })
        .collect();
    assert!(
        !banksels.is_empty(),
        "banking must insert at least one BANKSEL (got none):\n{asm}"
    );
}

#[test]
fn banked_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/banked.c",
            "-o",
            "tests/fixtures/banked.hex",
            "--map",
            "tests/fixtures/banked.map",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string("tests/fixtures/banked.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/banked.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/banked.map");
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(2_000_000);
    assert_eq!(p.ram()[out_addr], 255, "sum 1..90 = 4095, low byte 0xFF");
    assert!(p.halted());
}
