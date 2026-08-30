//! Local-array alloca acceptance for #149: stack `[N x i8]` written and
//! read at a runtime index, the local counterpart to `array.c`. The
//! driver HEX must run to halt on the simulator with `out == 4` for
//! `in == 3` (buf[3] = 4, then out = buf[3]).

use std::collections::HashMap;
use std::process::Command;

fn local_array_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/local_array.c"),
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
fn local_array_runs_correctly() {
    let layout = local_array_layout();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/local_array.c",
            "-o",
            "tests/fixtures/local_array.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/local_array.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 3; // in low byte = 3 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 4, "out == buf[3] == 3+1");
    assert!(p.halted());
}
