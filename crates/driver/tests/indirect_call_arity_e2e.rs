//! epic-cc#152 acceptance: an indirect call site's candidate set must be
//! filtered by argument count. The 1-arg RB-style site (`g_cb1(0x55)`)
//! must not collect the 0-arg `on_a` callback, or isel panics copying the
//! i8 arg into a param-less callee's slots.
//!
//! The e2e fires the interrupt mid-run and checks that the 1-arg site
//! dispatched `on_b` (out = 0x55) and the 0-arg site dispatched `on_a`
//! (out = 1), then main's SLEEP halts the machine.
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

#[test]
fn arity_filtered_indirect_call_dispatches_both_sites() {
    let hex_path = "tests/fixtures/indirect_call_arity.hex";
    let map_path = "tests/fixtures/indirect_call_arity.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call_arity.c",
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
    let out_addr = map_addr(&map, "out1");
    let out2_addr = map_addr(&map, "out2");
    let cb1_addr = map_addr(&map, "g_cb1");
    let cb0_addr = map_addr(&map, "g_cb0");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);

    // Run main to the point right after the two callback stores (g_cb1 =
    // on_b, g_cb0 = on_a), before main's RETURN and __start's SLEEP. The
    // ISR preempts main there.
    let mut steps = 0usize;
    while p.ram()[cb1_addr] == 0 || p.ram()[cb0_addr] == 0 {
        p.step();
        steps += 1;
        assert!(
            steps < 200,
            "never reached the callback stores (pc={})",
            p.pc()
        );
    }

    // Fire the interrupt: the ISR runs both sites (the 1-arg site must
    // dispatch on_b, the 0-arg site on_a) and RETFIE returns to main,
    // whose __start SLEEP halts the machine.
    p.fire_interrupt();
    p.run(200_000);
    assert!(p.halted(), "machine must halt after the ISR returns");
    assert_eq!(p.ram()[out_addr], 0x55, "1-arg site dispatched on_b");
    assert_eq!(p.ram()[out2_addr], 1, "0-arg site dispatched on_a");
    let _ = std::fs::remove_file(hex_path);
}
