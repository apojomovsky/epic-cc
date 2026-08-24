//! epic-cc#114 acceptance: a const struct table in flash read through a
//! ccp_sel-style pointer select with a runtime instance index.
//!
//! Hand-computed expectations (sim sets `inst` before run):
//!   - addrs[0] = { 0x15, 0x16, 0x17, 0x01 }
//!   - addrs[1] = { 0x1B, 0x1C, 0x1D, 0x02 }
//!   - inst = 0: out_* = 0x15/0x16/0x17/0x01 (inlined select path) and
//!     out_*2 = the same (sunk call-return path)
//!   - inst = 1: out_* = 0x1B/0x1C/0x1D/0x02, out_*2 = the same
//! The table stays `const` in source and must land in flash: the alloc map
//! classifies `addrs` as a const global with no RAM address.

use std::collections::HashMap;
use std::process::Command;

fn ccp_addrs_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/const_ccp_addrs.c"),
        &driver::clang::Options::default(),
    );

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let mut addrs: HashMap<String, u16> = HashMap::new();
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn const_ccp_addrs_selects_run_correctly() {
    let layout = ccp_addrs_layout();
    let addr = |n: &str| *layout.globals.get(n).expect(n) as usize;

    // The table is const (flash): no RAM allocation, classified as const.
    assert!(
        !layout.globals.contains_key("addrs"),
        "addrs must stay in flash (no RAM address)"
    );
    assert!(
        layout.const_globals.contains("addrs"),
        "addrs must be classified as a const global"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_ccp_addrs.c",
            "-o",
            "tests/fixtures/const_ccp_addrs.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/const_ccp_addrs.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);

    // inst = 0 -> element 0 (0x15, 0x16, 0x17, 0x01), both the inlined
    // select and the sunk call-return path.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("inst")] = 0;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_cprl")], 0x15, "cprl (inline) inst=0");
    assert_eq!(p.ram()[addr("out_cprh")], 0x16, "cprh (inline) inst=0");
    assert_eq!(p.ram()[addr("out_con")], 0x17, "con (inline) inst=0");
    assert_eq!(p.ram()[addr("out_irq")], 0x01, "irq (inline) inst=0");
    assert_eq!(p.ram()[addr("out_cprl2")], 0x15, "cprl (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_cprh2")], 0x16, "cprh (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_con2")], 0x17, "con (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_irq2")], 0x01, "irq (sunk) inst=0");

    // inst = 1 -> element 1 (0x1B, 0x1C, 0x1D, 0x02). Reset the machine (the
    // first run ended in SLEEP, so a continued run would not re-enter main).
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("inst")] = 1;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_cprl")], 0x1B, "cprl (inline) inst=1");
    assert_eq!(p.ram()[addr("out_cprh")], 0x1C, "cprh (inline) inst=1");
    assert_eq!(p.ram()[addr("out_con")], 0x1D, "con (inline) inst=1");
    assert_eq!(p.ram()[addr("out_irq")], 0x02, "irq (inline) inst=1");
    assert_eq!(p.ram()[addr("out_cprl2")], 0x1B, "cprl (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_cprh2")], 0x1C, "cprh (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_con2")], 0x1D, "con (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_irq2")], 0x02, "irq (sunk) inst=1");
}
