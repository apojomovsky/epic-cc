//! Milestone-13 acceptance: an interrupt-driven program — SFR access via
//! `inttoptr` (PORTB at absolute 0x06), a noinline shared helper `bump()`
//! duplicated for the ISR (`bump_isr`), the vector entry at word 4 with the
//! save/restore prologue/epilogue and RETFIE — compiles through the whole
//! driver pipeline and runs correctly in the simulator with the interrupt
//! fired mid-run. Acceptance: `in == 0x10` -> `out == 0x16`, PORTB
//! (RAM[0x06]) == 0x22, halted.
//!
//! `in` and `out` are volatile globals at 0x21 / 0x20 (the alloc layout the
//! driver used); PORTB is the F877A SFR at RAM[0x06].
//!
//! The injection point is main's **word 75** (`%2 = load out`, the argument
//! load of `out = bump(out)`, immediately after the `PORTB = 0x11` store at
//! word 74) — verified against the exact emitted asm (crates/asm/tests/
//! fixtures/interrupt.asm, which was captured from this same driver
//! pipeline): the ISR preempts main before the shared helper's argument is
//! read, so the ISR's bump lands in `out` before main's bump reads it.
//!
//! Hand computation from the emitted IR + the injection point (in = 0x10):
//!   main: out = in                          -> 0x10   (word 72 store)
//!   main: PORTB = 0x11                      (word 74 store)
//!   <- fire_interrupt at pc == 75: push 76, jump to the vector (word 4)
//!   isr:  save W/STATUS/PCLATH/FSR/retval -> 0x75-0x7C
//!         PORTB = 0x55                      (SFR write from the ISR)
//!         out = bump_isr(out = 0x10)        -> 0x11   (the _isr duplicate)
//!         restore; RETFIE -> pc == 76
//!   main: %2 = load out (0x11, the ISR's bump) -> bump(0x11) = 0x12 -> out
//!   main: %4 = load out (0x12); %5 = %4 + 1 = 0x13 -> out
//!   main: %6 = load out (0x13); %7 = bump(2) = 3; %8 = %6 + %7 = 0x16 -> out
//!   main: PORTB = 0x22; RETURN; __start: SLEEP -> halted
//! Final: out == 0x16, PORTB == 0x22 (the ISR's mid-run 0x55 is
//! overwritten by main's final SFR write), halted. The no-interrupt run
//! gives out == 0x15, so the ISR's bump is observable in the final value.
use std::collections::HashMap;
use std::process::Command;

/// The interrupt vector (word 4) and the injection point (word 75) as
/// documented above.
const VECTOR: u16 = 4;
const INJECT_PC: u16 = 75;

fn interrupt_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/interrupt.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, 8);
    let layout = alloc::allocate(&m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&m, &addrs);
    let asm = banking::assign_banks(&asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn interrupt_runs_correctly_with_mid_run_fire() {
    let layout = interrupt_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/interrupt.c", "tests/fixtures/interrupt.hex"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/interrupt.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 0x10; // in = 0x10

    // Run main to the injection point (word 75): the `%2 = load out` for
    // `out = bump(out)`, right after the `PORTB = 0x11` store.
    let mut steps = 0usize;
    while p.pc() != INJECT_PC {
        p.step();
        steps += 1;
        assert!(steps < 200, "never reached the injection point (pc = {})", p.pc());
    }
    // The pre-ISR state the hand computation starts from.
    assert_eq!(p.ram()[out_addr], 0x10, "out == in before the ISR");
    assert_eq!(p.ram()[0x06], 0x11, "PORTB == 0x11 (main's SFR write) before the ISR");

    // Fire the interrupt: push pc+1 (76), jump to the vector at word 4.
    p.fire_interrupt();
    assert_eq!(p.pc(), VECTOR, "the ISR starts at the vector (word 4)");

    // The ISR runs (PORTB = 0x55, out = bump_isr(out)), RETFIE returns to
    // word 76, and main completes: out == 0x16, PORTB == 0x22, then the
    // __start SLEEP halts the machine.
    p.run(500_000);
    assert_eq!(p.ram()[out_addr], 0x16, "out == hand-computed 0x16 (ISR bump 0x10 -> 0x11, then 0x11 -> 0x12 -> 0x13 -> 0x16)");
    assert_eq!(p.ram()[0x06], 0x22, "PORTB == 0x22 (main's final SFR write)");
    assert!(p.halted());
}
