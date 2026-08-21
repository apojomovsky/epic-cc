//! Issue #15 acceptance: an interrupt-gating program compiles through the
//! whole driver pipeline and the simulator honours INTCON. A request made
//! while GIE is clear stays pending; it is taken only once main unmasks, and
//! it is taken exactly once. See `fixtures/interrupt_gate.c`.
use std::collections::HashMap;
use std::process::Command;

fn gate_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/interrupt_gate.c",
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
fn a_masked_request_is_deferred_until_main_sets_gie() {
    let layout = gate_layout();
    let addr = |g: &str| *layout.globals.get(g).unwrap_or_else(|| panic!("no global {g}")) as usize;
    let (stage, isr_ran) = (addr("stage"), addr("isr_ran"));

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/interrupt_gate.c", "-o", "tests/fixtures/interrupt_gate.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/interrupt_gate.hex").unwrap();
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));

    // Run into the masked window (stage == 1) and request the interrupt.
    let mut steps = 0;
    while p.ram()[stage] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "never reached stage 1");
    }
    p.request_interrupt();
    assert!(p.interrupt_pending(), "the request latches while GIE is clear");
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
        assert_eq!(p.ram()[isr_ran], 0, "the handler ran while interrupts were masked");
    }
    assert!(p.interrupt_pending(), "still pending at stage 2, before GIE goes up");

    // Main sets GIE; the pending request is taken, once.
    p.run(500_000);
    assert!(p.halted(), "program must SLEEP-halt, not spin in the handler");
    assert_eq!(p.ram()[isr_ran], 1, "the handler runs exactly once after unmasking");
    assert_eq!(p.ram()[stage], 3, "main reaches its final stage");
    assert!(!p.interrupt_pending(), "the latch was consumed");
}
