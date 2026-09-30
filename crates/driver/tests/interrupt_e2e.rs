//! Milestone-13 acceptance: an interrupt-driven program — SFR access via
//! `inttoptr` (PORTB at absolute 0x06), a noinline shared helper `bump()`
//! duplicated for the ISR (`bump_isr`), the vector entry at word 4 with the
//! save/restore prologue/epilogue and RETFIE — compiles through the whole
//! driver pipeline and runs correctly in the simulator with the interrupt
//! fired mid-run. Acceptance: `in == 0x10` -> `out == 0x16`, PORTB
//! (RAM[0x06]) == 0x22, halted.
//!
//! `in` and `out` are volatile globals (addresses read off `--map`
//! below); PORTB is the F877A SFR at RAM[0x06].
//!
//! The injection point is main's **word 72** (`%2 = load out`, the argument
//! load of `out = bump(out)`, immediately after the `PORTB = 0x11` store at
//! word 71), verified against the exact emitted asm (crates/asm/tests/
//! fixtures/interrupt.asm, which was captured from this same driver
//! pipeline): the ISR preempts main before the shared helper's argument is
//! read, so the ISR's bump lands in `out` before main's bump reads it.
//!
//! Hand computation from the emitted IR + the injection point (in = 0x10):
//!   main: out = in                          -> 0x10   (word 69 store)
//!   main: PORTB = 0x11                      (word 71 store)
//!   <- fire_interrupt at pc == 72: push 72, jump to the vector (word 4)
//!   isr:  save W/STATUS/PCLATH/FSR/retval/scratch -> 0x75-0x7D
//!         PORTB = 0x55                      (SFR write from the ISR)
//!         out = bump_isr(out = 0x10)        -> 0x11   (the _isr duplicate)
//!         restore; RETFIE -> pc == 72
//!   main: %2 = load out (0x11, the ISR's bump) -> bump(0x11) = 0x12 -> out
//!   main: %4 = load out (0x12); %5 = %4 + 1 = 0x13 -> out
//!   main: %6 = load out (0x13); %7 = bump(2) = 3; %8 = %6 + %7 = 0x16 -> out
//!   main: PORTB = 0x22; RETURN; __start: SLEEP -> halted
//! Final: out == 0x16, PORTB == 0x22 (the ISR's mid-run 0x55 is
//! overwritten by main's final SFR write), halted. The no-interrupt run
//! gives out == 0x15, so the ISR's bump is observable in the final value.
use std::process::Command;

/// The interrupt vector (word 4) and the injection point (word 72) as
/// documented above.
const VECTOR: u16 = 4;
const INJECT_PC: u16 = 72;

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
fn interrupt_runs_correctly_with_mid_run_fire() {
    let hex_path = "tests/fixtures/interrupt.hex";
    let map_path = "tests/fixtures/interrupt.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/interrupt.c",
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
    let in_addr = map_addr(&map, "in");
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 0x10; // in = 0x10

    // Run main to the injection point (word 72): the `%2 = load out` for
    // `out = bump(out)`, right after the `PORTB = 0x11` store.
    let mut steps = 0usize;
    while p.pc() != INJECT_PC {
        p.step();
        steps += 1;
        assert!(
            steps < 200,
            "never reached the injection point (pc = {})",
            p.pc()
        );
    }
    // The pre-ISR state the hand computation starts from.
    assert_eq!(p.ram()[out_addr], 0x10, "out == in before the ISR");
    assert_eq!(
        p.ram()[0x06],
        0x11,
        "PORTB == 0x11 (main's SFR write) before the ISR"
    );
    // Fire the interrupt: push pc (72), jump to the vector at word 4.
    p.fire_interrupt();
    assert_eq!(p.pc(), VECTOR, "the ISR starts at the vector (word 4)");

    // The ISR runs (PORTB = 0x55, out = bump_isr(out)), RETFIE returns to
    // word 72, and main completes: out == 0x16, PORTB == 0x22, then the
    // __start SLEEP halts the machine.
    p.run(500_000);
    assert_eq!(
        p.ram()[out_addr],
        0x16,
        "out == hand-computed 0x16 (ISR bump 0x10 -> 0x11, then 0x11 -> 0x12 -> 0x13 -> 0x16)"
    );
    assert_eq!(
        p.ram()[0x06],
        0x22,
        "PORTB == 0x22 (main's final SFR write)"
    );
    assert!(p.halted());
}
