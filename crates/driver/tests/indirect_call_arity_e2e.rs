//! epic-cc#152 acceptance: an indirect call site's candidate set must be
//! filtered by argument count. The 1-arg RB-style site (`g_cb1(0x55)`)
//! must not collect the 0-arg `on_a` callback, or isel panics copying the
//! i8 arg into a param-less callee's slots.
//!
//! The e2e fires the interrupt mid-run and checks that the 1-arg site
//! dispatched `on_b` (out = 0x55) and the 0-arg site dispatched `on_a`
//! (out = 1), then main's SLEEP halts the machine.
use std::collections::HashMap;
use std::process::Command;

fn layout_for() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/indirect_call_arity.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
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
fn arity_filtered_indirect_call_dispatches_both_sites() {
    let layout = layout_for();
    let out_addr = *layout.globals.get("out1").expect("out1") as usize;
    let out2_addr = *layout.globals.get("out2").expect("out2") as usize;
    let cb1_addr = *layout.globals.get("g_cb1").expect("g_cb1") as usize;
    let cb0_addr = *layout.globals.get("g_cb0").expect("g_cb0") as usize;

    let hex_path = "tests/fixtures/indirect_call_arity.hex";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call_arity.c",
            "-o",
            hex_path,
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
