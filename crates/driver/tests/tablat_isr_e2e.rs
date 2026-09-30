//! epic-cc#532 regression: an ISR that performs a const (flash) read must
//! not corrupt the preempted main context's in-flight TBLRD value.
//!
//! `TBLRD*` leaves the flash byte in TABLAT and the consumer `MOVFF 0xFF5,
//! dst` is the very next instruction, so the byte is live across one
//! instruction boundary. ADR-013 saves TBLPTR for this same read sequence
//! but TABLAT was in no save set; before this fix a handler whose own const
//! read emits TBLRD resumed main against the handler's byte.
use std::process::Command;

/// Globals' RAM addresses, read off the compiler's own `--map` output.
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts (see array_e2e's `map_addr`).
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn run() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/tablat_isr.c",
            "-o",
            "tests/fixtures/tablat_isr.hex",
            "--map",
            "tests/fixtures/tablat_isr.map",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm_out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/tablat_isr.c",
            "-o",
            "tests/fixtures/tablat_isr.asm",
            "--device",
            "p18f4550",
            "--emit",
            "asm",
        ])
        .output()
        .expect("run driver for asm");
    assert!(
        asm_out.status.success(),
        "driver asm: {}",
        String::from_utf8_lossy(&asm_out.stderr)
    );
    let asm = std::fs::read_to_string("tests/fixtures/tablat_isr.asm").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/tablat_isr.asm");
    let map = std::fs::read_to_string("tests/fixtures/tablat_isr.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/tablat_isr.map");
    let sym = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string("tests/fixtures/tablat_isr.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);

    // Both contexts must actually read flash, or this proves nothing.
    assert!(
        asm.matches("TBLRD").count() >= 2,
        "both main and the handler must emit a const read:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFF5"),
        "the const read's consumer moves TABLAT:\n{asm}"
    );

    // idx/isr_idx arrive via the fixture's init stores (epic-cc#561):
    // __start clears zero-initialized globals, so sim-side seeds would
    // not survive.

    // Interrupt at the instruction right after main's TBLRD*: TABLAT holds
    // main's 0x22 and the consumer has not run yet.
    let mut steps = 0;
    let mut fired = false;
    while steps < 4000 {
        p.step();
        steps += 1;
        if p.ram()[0xFF5] == 0x22 && p.ram()[sym("out")] == 0 {
            p.fire_interrupt();
            p.run(20_000);
            fired = true;
            break;
        }
    }
    assert!(fired, "never observed TABLAT holding main's byte");

    assert_eq!(
        p.ram()[sym("out")],
        0x22,
        "main's const read must survive a handler that reads flash too"
    );
    assert_eq!(
        p.ram()[sym("isr_out")],
        0x88,
        "the handler's own read is 0x88"
    );
}

#[test]
fn isr_const_read_preserves_the_preempted_tblrd_value() {
    run();
}
