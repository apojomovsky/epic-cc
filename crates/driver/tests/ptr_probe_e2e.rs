//! Milestone-5 pointer/const acceptance: the spike's probe (docs/11) — a
//! runtime RAM pointer AND a const-table read in one program. Acceptance:
//! (a) the emitted .asm engages both lowerings — `CALL __read_table` (the
//! RETLW const-table reader) and `MOVWF FSR` (the FSR/INDF RAM indirect
//! path); (b) the driver's HEX runs to halt in the simulator with
//! `out == 20` for `in == 1` (ram[1] = table[1] = 20, then out = ram[1]).
//!
//! `in` is a 16-bit global at 0x20-0x21 (its low byte holds the input), the
//! const `table` gets no RAM address, and `out`/`ram` addresses are read from
//! the same alloc layout the driver used.

use std::collections::HashMap;
use std::process::Command;

/// Run clang + the full IR pipeline on the ptr_probe fixture, exactly as the
/// driver does, and return the alloc layout plus the final (banked,
/// peepholed) .asm.
fn ptr_probe_pipeline() -> (alloc::AllocLayout, String) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            "tests/fixtures/ptr_probe.c",
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let ll_text = String::from_utf8(ll.stdout).unwrap();

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
    let (layout, _) = ptr_probe_pipeline();
    // `out` is a global; read its physical address from the same layout the
    // driver used so the simulator resolves the right bank.
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/ptr_probe.c",
            "-o",
            "tests/fixtures/ptr_probe.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/ptr_probe.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 1; // in low byte = 1 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 20, "out == table[1] == 20");
    assert!(p.halted());
}
