//! epic-cc#477 regression: an ISR that multiplies must not corrupt the
//! preempted main context's in-flight PRODL/PRODH product.
//!
//! The `__mul_*` routines write MULWF and read PRODL/PRODH several
//! instructions later, so the product is live across an interruptible
//! window. Before this fix the compat ISR saved neither byte, and an
//! interrupt taken inside that window made main resume against the ISR's
//! product: a silent wrong answer (out == the ISR's result).
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
            "tests/fixtures/prod_isr.c",
            "-o",
            "tests/fixtures/prod_isr.hex",
            "--map",
            "tests/fixtures/prod_isr.map",
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
    let map = std::fs::read_to_string("tests/fixtures/prod_isr.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/prod_isr.map");
    let sym = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string("tests/fixtures/prod_isr.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    // Inputs arrive via the fixture's init stores (epic-cc#561): __start
    // clears zero-initialized globals, so sim-side seeds would not survive.

    // Run until the routine has written main's first partial product to
    // PRODL and main has not yet stored its result, then interrupt there:
    // the window where PROD holds main's live value.
    let main_product = 47u16.wrapping_mul(5) & 0xFF;
    let mut steps = 0;
    while steps < 400 {
        p.step();
        steps += 1;
        if p.ram()[0xFF3] == main_product as u8 && p.ram()[sym("out")] == 0 {
            p.fire_interrupt();
            p.run(4000);
            break;
        }
    }
    assert_eq!(
        p.ram()[sym("out")],
        235,
        "main's product must survive an ISR that multiplies (got {} from the ISR's {})",
        p.ram()[sym("out")],
        p.ram()[sym("isr_out")],
    );
    assert_eq!(
        p.ram()[sym("isr_out")],
        21,
        "the ISR's own product must be 21"
    );
}

#[test]
fn isr_multiply_preserves_the_preempted_product() {
    run();
}
