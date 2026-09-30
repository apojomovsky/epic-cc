// epic-cc#393 acceptance: the same real-world ISR shape as
// indirect_call_isr_e2e.rs (a callback stored in a struct, invoked through a
// pointer, from inside an interrupt handler -- exercising a genuine
// function-call return value, an SFR write, and a struct field store/load),
// this time compiled for PIC16F74 (docs/39 D-2: two non-aliased GPR regions,
// no common_ram, isr_w_shadow + isr_home_window instead). This is the actual
// proof the mechanism works end to end through the real compiler, not just
// the hand-assembled shadow-slot sequence crates/sim/tests/
// interrupts_no_common_ram.rs validated in isolation, nor the synthetic
// unit-level banking proof in crates/banking/tests/banking.rs.
//
// Hand computation (in = 0x10), identical to the PIC16F877A version:
//   main: out = in                        -> 0x10
//   main: PORTB = 0x11
//   <- ISR fires here
//   ISR:  PORTB = 0x55; g_dev.cb = on_event_isr; out = on_event_isr(in=0x10) -> 0x11
//   main: out = on_event(out=0x11)        -> 0x12
//   main: PORTB = 0x22
//   out == 0x12, PORTB == 0x22, halted.

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

const VECTOR: u16 = 4;

#[test]
fn isr_fires_callback_through_function_pointer_on_p16f74() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call_isr.c",
            "-o",
            "tests/fixtures/indirect_call_isr_p16f74.hex",
            "--map",
            "tests/fixtures/indirect_call_isr_p16f74.map",
            "--device",
            "p16f74",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hex = std::fs::read_to_string("tests/fixtures/indirect_call_isr_p16f74.hex").unwrap();
    let map =
        std::fs::read_to_string("tests/fixtures/indirect_call_isr_p16f74.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/indirect_call_isr_p16f74.map");
    let in_addr = map_addr(&map, "in");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::with_device(&device::PIC16F74, prog);
    p.ram_mut()[in_addr] = 0x10; // in = 0x10

    // Run main up to the point right after the `PORTB = 0x11` store, before
    // `out = on_event(out)`. The ISR preempts main there.
    let mut steps = 0usize;
    while p.ram()[0x06] != 0x11 {
        p.step();
        steps += 1;
        assert!(
            steps < 200,
            "never reached the PORTB=0x11 store (pc={})",
            p.pc()
        );
    }

    assert_eq!(p.ram()[out_addr], 0x10, "out == in before the ISR");

    // Fire the interrupt: push pc+1, jump to the vector at word 4.
    p.fire_interrupt();
    assert_eq!(p.pc(), VECTOR, "the ISR starts at the vector (word 4)");

    // The ISR runs (PORTB = 0x55, stores on_event_isr, invokes it -> out =
    // 0x11), RETFIE returns to main, and main completes: out == 0x12,
    // PORTB == 0x22, then the __start SLEEP halts the machine -- the real
    // proof the D-2 mechanism works through the actual compiler, not just
    // in isolation (crates/sim/tests/interrupts_no_common_ram.rs) or at
    // the banking unit level (crates/banking/tests/banking.rs).
    p.run(500_000);
    assert_eq!(
        p.ram()[out_addr],
        0x12,
        "out == 0x12 (ISR callback 0x10 -> 0x11, then main's on_event 0x11 -> 0x12)"
    );
    assert_eq!(
        p.ram()[0x06],
        0x22,
        "PORTB == 0x22 (main's final SFR write)"
    );
    assert!(p.halted());
}
