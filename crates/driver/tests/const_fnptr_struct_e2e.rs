//! epic-cc#154 acceptance: a `static const` struct with a function-pointer
//! field (a table-driven FSM's transition rows) must decode and dispatch.
//!
//! The e2e compiles the fixture, runs it in the sim with `g_idx` set to
//! each row, and asserts the guard dispatch: row 0's guard is non-null and
//! returns 1 (g_count < 2), row 1's guard is null and out stays 0.
use std::collections::HashMap;
use std::process::Command;

fn layout_for() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/const_fnptr_struct.c"),
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
fn const_fnptr_struct_dispatches_the_guard() {
    let layout = layout_for();
    let idx_addr = *layout.globals.get("g_idx").expect("g_idx") as usize;
    let out_addr = *layout.globals.get("out").expect("out") as usize;

    let hex_path = "tests/fixtures/const_fnptr_struct.hex";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_fnptr_struct.c",
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

    // Row 0: guard non-null -> out = guard(0) = 1.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[idx_addr] = 0;
    p.run(200_000);
    assert!(p.halted(), "row 0 must halt");
    assert_eq!(p.ram()[out_addr], 1, "row 0 guard dispatched");

    // Row 1: guard null -> out stays 0.
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[idx_addr] = 1;
    p.run(200_000);
    assert!(p.halted(), "row 1 must halt");
    assert_eq!(p.ram()[out_addr], 0, "row 1 guard is null");
    let _ = std::fs::remove_file(hex_path);
}
