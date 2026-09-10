//! Priority-interrupt acceptance (epic-cc#346): a high (`__interrupt(1)`)
//! and a low (`__interrupt(2)`) handler sharing the noinline helper
//! `bump()` with main and each other (fixture `priority_irq.c`), compiled
//! through the whole driver pipeline for the PIC18F4550 and run in the
//! simulator with nested fires.
//!
//! Acceptance: `ticks == 2`, `pkts == 2`, `main_ctr == 3`,
//! `hi_saw_lo == 1`, halted.
//!
//! The injection is deterministic, not pc-pinned: run main until
//! `main_ctr == 1`, fire low, then step until `lo_flag == 1` and fire
//! high. The high fire lands between the low handler's `lo_flag = 1` and
//! `lo_flag = 0` stores by construction (the sim is single-stepped), so
//! `hi_saw_lo == 1` proves the high ISR ran while the low one was live —
//! nesting, not sequential service. The multi-call bodies (`ticks == 2`
//! through two `bump` calls under preemption) prove the three frame
//! regions are disjoint: any clobber would corrupt a counter.
use std::process::Command;

fn priority_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    // Mirror the driver's own clang options: the fixture uses the
    // `__interrupt(n)` keyword, which rides as a `-D` in `xc8_predefines`.
    let opts = driver::clang::Options {
        defines: driver::predef::xc8_predefines(device::Core::Pic18, device::PIC18F4550.name),
        ..Default::default()
    };
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/priority_irq.c"),
        &opts,
    );

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, device::PIC18F4550.stack_depth as usize);
    alloc::allocate(&device::PIC18F4550, &m, &callgraph::edges_text(&cg))
}

#[test]
fn priority_interrupts_nest_with_disjoint_frames() {
    let layout = priority_layout();
    let addr = |name: &str| *layout.globals.get(name).expect("global") as usize;
    let (ticks, pkts, main_ctr, lo_flag, hi_saw_lo) = (
        addr("ticks"),
        addr("pkts"),
        addr("main_ctr"),
        addr("lo_flag"),
        addr("hi_saw_lo"),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/priority_irq.c",
            "-o",
            "tests/fixtures/priority_irq.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/priority_irq.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);

    // Run main into its loop body (first `main_ctr` bump landed).
    let mut steps = 0usize;
    while p.ram()[main_ctr] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "main never reached its loop");
    }

    // Fire low: the handler starts at the low vector with GIEH still set.
    p.fire_low_interrupt();
    assert_eq!(p.pc(), 0x0018, "low ISR starts at vector 0x0018");

    // Step the low handler to its `lo_flag = 1` store, then fire high:
    // the high ISR runs while the low one is live, by construction.
    steps = 0;
    while p.ram()[lo_flag] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "low ISR never raised its flag");
    }
    p.fire_interrupt();
    assert_eq!(p.pc(), 0x0008, "high ISR preempts at vector 0x0008");

    // Both handlers drain, main completes, `__start` sleeps.
    p.run(500_000);
    assert_eq!(p.ram()[ticks], 2, "high body: 0 -> bump -> bump = 2");
    assert_eq!(p.ram()[pkts], 2, "low body survived preemption: 2");
    assert_eq!(p.ram()[main_ctr], 3, "main's state survived both ISRs");
    assert_eq!(
        p.ram()[hi_saw_lo],
        1,
        "high ran while low was live (nesting, not sequential)"
    );
    assert!(p.halted());
}
