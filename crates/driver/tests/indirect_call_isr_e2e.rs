// epic-cc#73 acceptance: an ISR fires a callback through a function pointer
// stored in a struct. The callback is a shared function, so legalize
// duplicates it as `_isr` and the ISR's stored pointer must reference the
// copy (which runs in the disjoint ISR region). The e2e fires the interrupt
// mid-run and checks the ISR's callback ran and wrote the expected value.
//
// Hand computation (in = 0x10):
//   main: out = in                        -> 0x10
//   main: PORTB = 0x11
//   <- ISR fires here
//   ISR:  PORTB = 0x55; g_dev.cb = on_event_isr; out = on_event_isr(in=0x10) -> 0x11
//   main: out = on_event(out=0x11)        -> 0x12
//   main: PORTB = 0x22
//   out == 0x12, PORTB == 0x22, halted.

use std::collections::HashMap;
use std::process::Command;

const VECTOR: u16 = 4;

fn isr_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/indirect_call_isr.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, 8);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn isr_fires_callback_through_function_pointer() {
    let layout = isr_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call_isr.c",
            "-o",
            "tests/fixtures/indirect_call_isr.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/indirect_call_isr.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 0x10; // in = 0x10

    // Run main to the point right after the `PORTB = 0x11` store, before the
    // `out = on_event(out)` call. The ISR preempts main there.
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
    // PORTB == 0x22, then the __start SLEEP halts the machine.
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
