//! Milestone-5 pointer/const acceptance: the spike's probe (docs/11) — a
//! runtime RAM pointer AND a const-table read in one program. Acceptance:
//! (a) the emitted .asm engages both lowerings — `CALL __read_table` (the
//! RETLW const-table reader) and `MOVWF FSR` (the FSR/INDF RAM indirect
//! path); (b) the driver's HEX runs to halt in the simulator with
//! `out == 20` for `in == 1` (ram[1] = table[1] = 20, then out = ram[1]).
//!
//! `in` is a 16-bit global at 0x20-0x21 (its low byte holds the input), the
//! const `table` gets no RAM address, and `out`'s address is read off the
//! driver's `--map` output in the run test below.

use std::collections::HashMap;
use std::process::Command;

/// `out`'s RAM address, read off the compiler's own `--map`
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

/// Run clang + the full IR pipeline on the ptr_probe fixture, exactly as the
/// driver does, and return the alloc layout plus the final (banked,
/// peepholed) .asm.
fn ptr_probe_pipeline() -> (alloc::AllocLayout, String) {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/ptr_probe.c"),
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
    (layout, peephole::optimize(&asm))
}

#[test]
fn ptr_probe_engages_both_pointer_lowerings() {
    let (_, asm) = ptr_probe_pipeline();
    assert!(
        asm.contains("CALL __read_table"),
        "const-table read must call the RETLW reader:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF FSR"),
        "RAM indirect access must set FSR:\n{asm}"
    );
}

#[test]
fn ptr_probe_runs_correctly() {
    let hex_path = "tests/fixtures/ptr_probe.hex";
    let map_path = "tests/fixtures/ptr_probe.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/ptr_probe.c",
            "-o",
            hex_path,
            "--map",
            map_path,
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

    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(map_path);
    // `out` is a global; read its physical address off `--map` so the
    // simulator resolves the right bank.
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 1; // in low byte = 1 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 20, "out == table[1] == 20");
    assert!(p.halted());
}
