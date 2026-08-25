//! epic-cc#133 acceptance: a straight-line program with the pid clamp
//! pattern: signed min/max on i16 (folded by clang -O1 into
//! `llvm.smax`/`llvm.smin` intrinsic calls), then a 16x16 -> 32 signed
//! multiply (folded to `llvm.abs` intrinsics + a `mul i32`) feeding an
//! i16 truncate: compiles through the whole driver pipeline and runs
//! correctly in the simulator.
//!
//! `in_a`, `in_min`, `in_max`, `in_b`, `out` are globals; their addresses
//! come from the same alloc layout the driver used.
//!
//! Hand computation (in_a = -3000, in_min = -1000, in_max = 1000,
//! in_b = 15), traced against the emitted IR in fixtures/pid_clamp.c:
//!   clamp(-3000, -1000, 1000) = -1000      (smax i16, then smin i16)
//!   mul_s16(-1000, 15): |a|,|b| abs -> 1000, 15; product = 15000;
//!     signs differ -> negate = -15000       (abs i16 x2 + mul nuw i32 + select)
//!   p >> 8 = -15000 >> 8 = -59 (arithmetic shift), trunc i32 -> i16 = -59
//!   out = -59 = 0xFFC5
//!
//! The mul i32 lowers to a `CALL __mul_u32` runtime routine (no hardware
//! multiply on PIC14), and the `llvm.abs` intrinsic's `i1 false` immarg
//! exercises the irparse call-arg fix.

use std::collections::HashMap;
use std::process::Command;

fn pid_clamp_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/pid_clamp.c"),
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
fn pid_clamp_runs_correctly() {
    let layout = pid_clamp_layout();
    let a_addr = *layout.globals.get("in_a").expect("in_a global") as usize;
    let min_addr = *layout.globals.get("in_min").expect("in_min global") as usize;
    let max_addr = *layout.globals.get("in_max").expect("in_max global") as usize;
    let b_addr = *layout.globals.get("in_b").expect("in_b global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/pid_clamp.c",
            "-o",
            "tests/fixtures/pid_clamp.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/pid_clamp.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    let i16 = |v: i16| (v as u16).to_le_bytes();
    p.ram_mut()[a_addr..a_addr + 2].copy_from_slice(&i16(-3000));
    p.ram_mut()[min_addr..min_addr + 2].copy_from_slice(&i16(-1000));
    p.ram_mut()[max_addr..max_addr + 2].copy_from_slice(&i16(1000));
    p.ram_mut()[b_addr..b_addr + 2].copy_from_slice(&i16(15));
    p.run(500_000);
    let got = (p.ram()[out_addr] as u16) | ((p.ram()[out_addr + 1] as u16) << 8);
    assert_eq!(
        got as i16, -59,
        "out == hand-computed -59 for the clamp + s16 mul"
    );
    assert!(p.halted());
}
