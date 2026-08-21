//! Milestone-7 structs acceptance: a program exercising the full M7 surface
//! — sret calls (`mk`), byval calls from globals (`sum`, `pick`), struct
//! copies (`g = mk(...)` -> volatile memcpy), dynamic array-in-struct
//! indexing (`arr.v[arr.n]`, `x.v[x.n]`), and nested-struct field math with
//! folded byte GEPs (`go.in.a / go.in.b / go.z` at offsets 0/2/4) — compiles
//! through the whole driver pipeline and runs correctly in the simulator.
//! Acceptance: `out == 0x4E` (hand-computed, see the trace in
//! fixtures/structs.c) and the machine halts.
//!
//! `out` is the i8 global at the address the same alloc layout the driver
//! used gives it.

use std::collections::HashMap;
use std::process::Command;

fn structs_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/structs.c",
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
fn structs_runs_correctly() {
    let layout = structs_layout();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/structs.c", "-o", "tests/fixtures/structs.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/structs.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 0x4E, "out == hand-computed 0x4E");
    assert!(p.halted());
}
