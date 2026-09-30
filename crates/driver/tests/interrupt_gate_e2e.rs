//! Issue #15 acceptance: an interrupt-gating program compiles through the
//! whole driver pipeline and the simulator honours INTCON. A request made
//! while GIE is clear stays pending; it is taken only once main unmasks, and
//! it is taken exactly once. See `fixtures/interrupt_gate.c`.
use std::process::Command;

/// `stage` and `isr_ran`'s RAM addresses, read off the compiler's own `--map`
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
fn a_masked_request_is_deferred_until_main_sets_gie() {
    let hex_path = "tests/fixtures/interrupt_gate.hex";
    let map_path = "tests/fixtures/interrupt_gate.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/interrupt_gate.c",
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
    let (stage, isr_ran) = (map_addr(&map, "stage"), map_addr(&map, "isr_ran"));
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));

    // Run into the masked window (stage == 1) and request the interrupt.
    let mut steps = 0;
    while p.ram()[stage] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "never reached stage 1");
    }
    p.request_interrupt();
    assert!(
        p.interrupt_pending(),
        "the request latches while GIE is clear"
    );
    assert_ne!(
        p.ram()[pic14_sim::INTCON] & pic14_sim::INTF,
        0,
        "the request raises the source flag even while masked"
    );

    // Through the whole masked window the handler must not run.
    steps = 0;
    while p.ram()[stage] != 2 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "never reached stage 2");
        assert_eq!(
            p.ram()[isr_ran],
            0,
            "the handler ran while interrupts were masked"
        );
    }
    assert!(
        p.interrupt_pending(),
        "still pending at stage 2, before GIE goes up"
    );

    // Main sets GIE; the pending request is taken, once.
    p.run(500_000);
    assert!(
        p.halted(),
        "program must SLEEP-halt, not spin in the handler"
    );
    assert_eq!(
        p.ram()[isr_ran],
        1,
        "the handler runs exactly once after unmasking"
    );
    assert_eq!(p.ram()[stage], 3, "main reaches its final stage");
    assert!(!p.interrupt_pending(), "the latch was consumed");
}
