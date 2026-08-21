//! Milestone-6 scalar acceptance: an ordinary embedded-C program whose loop
//! exercises the newly supported scalar surface — `sub`, `and i8`, `or`,
//! `xor`, and the `eq`/`ne`/`ugt`/`ult` icmp predicates — compiles through
//! the whole driver pipeline and runs correctly in the simulator. Acceptance:
//! for `in == 7` the hand-computed `out == 174` and the machine halts.
//!
//! `in` and `out` are i8 globals; their addresses are read from the same
//! alloc layout the driver used (see the trace in fixtures/scalar.c).

use std::collections::HashMap;
use std::process::Command;

fn scalar_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/scalar.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

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
fn scalar_runs_correctly() {
    let layout = scalar_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/scalar.c", "-o", "tests/fixtures/scalar.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/scalar.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 7; // in = 7
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 174, "out == hand-computed 174 for in == 7");
    assert!(p.halted());
}
