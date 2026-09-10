//! Float-preemption acceptance (epic-cc#357): main computes a float chain
//! (fixture `float_isr.c`) and a high-priority ISR fires mid-chain,
//! itself doing float math. Per-context float frames (each context's
//! routine copies place in its own overlay region) keep main's in-flight
//! operands and scratch intact, so main completes with the bit-exact
//! result the un-preempted run produces: 0x42082222 (34.0333328, RNE).
//!
//! The sim fires on a fixed step cadence (gated on GIEH): the float
//! recipes run hundreds of instructions, so of the 8 fires several land
//! INSIDE main's in-flight mul/div. The exact-result assertion is what
//! makes "mid-op" observable (a clobbered operand or scratch perturbs
//! the quotient's last bits).
use std::process::Command;

fn float_isr_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let opts = driver::clang::Options {
        defines: driver::predef::xc8_predefines(device::Core::Pic18, device::PIC18F4550.name),
        ..Default::default()
    };
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/float_isr.c"),
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
fn float_isr_preempts_main_mid_op_without_corruption() {
    let layout = float_isr_layout();
    let addr = |name: &str| *layout.globals.get(name).expect("global") as usize;
    let out = addr("out");

    let out_p = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/float_isr.c",
            "-o",
            "tests/fixtures/float_isr.hex",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        out_p.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out_p.stderr)
    );

    let hex = std::fs::read_to_string("tests/fixtures/float_isr.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);

    // Run with the ISR firing on the cadence (mid-chain).
    // Firmware owns INTCON: seed GIEH (bit 7) the way a real program
    // would before enabling its interrupt source. Fire on a fixed step
    // cadence, gated on GIEH (cleared inside the ISR, so no re-entry):
    // the float recipes run hundreds of instructions, so of the 8 fires
    // several land INSIDE main's in-flight mul/div (the exact hazard
    // epic-cc#357 removes).
    p.ram_mut()[0xFF2] = 0x80;
    let mut steps = 0usize;
    let mut fired = 0u32;
    while !p.halted() {
        if steps % 250 == 100 && fired < 8 && p.ram()[0xFF2] & 0x80 != 0 {
            p.fire_interrupt();
            fired += 1;
        }
        p.step();
        steps += 1;
        if steps > 5_000_000 {
            panic!("program never halted (fired = {fired}, pc = {:#X})", p.pc());
        }
    }
    assert_eq!(fired, 8, "the cadence must deliver all 8 firings");
    // out = 34.0333328 = 0x42082222, LE 22 22 08 42: bit-exact despite the
    // ISR running its own float adds inside main's divide.
    let got = u32::from_le_bytes([
        p.ram()[out],
        p.ram()[out + 1],
        p.ram()[out + 2],
        p.ram()[out + 3],
    ]);
    assert_eq!(got, 0x4208_2222, "main's float chain must be unperturbed");
}
