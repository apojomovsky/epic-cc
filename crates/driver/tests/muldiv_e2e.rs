//! Milestone-8 mul/div/mod/shift acceptance: a straight-line program
//! exercising the whole new scalar surface — mul, udiv, urem, sdiv, srem,
//! shl (const), lshr (const), and a variable-count shl — on both i16 and
//! i8 — compiles through the whole driver pipeline and runs correctly in
//! the simulator. Acceptance: for `in == 301` the hand-computed
//! `out == 210` and the machine halts.
//!
//! `in`, `out`, `gate` are globals; their addresses are read from the same
//! alloc layout the driver used (see the trace in fixtures/muldiv.c).
//!
//! Hand computation from the EXACT emitted IR (in = 301; the C in
//! fixtures/muldiv.c is shaped so clang -O1 keeps every op — see the
//! comments there):
//!   %1  = load volatile i16 @in                      301
//!   %2  = udiv i16 %1, 7                             43          (udiv i16)
//!   %3  = load volatile i16 @out                     43
//!   %4  = mul i16 %3, 3                              129         (mul i16)
//!   %5  = urem i16 %1, 5                             1           (urem i16)
//!   %6  = add i16 %4, %5                             130
//!   %8  = shl i16 %7, 2                              520         (shl const)
//!   %10 = lshr i16 %9, 3                             65          (lshr const)
//!   %11 = lshr i16 %1, 4                             18          (lshr const)
//!   %12 = or i16 %10, %11                            83
//!   %13 = add nsw i16 %1, -320                       -19
//!   %14 = sdiv i16 %13, -3                           6           (sdiv i16)
//!   %15 = srem i16 %13, 3                            -1          (srem i16)
//!   %17 = add i16 %16, %15                           5
//!   %19 = load volatile i8 @gate                     45 (trunc)
//!   %21 = mul i8 %19, 7                              (45*7)&0xFF = 59 (mul i8)
//!   %22 = udiv i8 %21, 3                             59/3 = 19   (udiv i8)
//!   %25 = add i16 %23, %24                           5 + 19 = 24
//!   %26 = mul i16 %25, 5                             120         (mul i16)
//!   %28 = and i16 %1, 3                              1
//!   %29 = shl i16 %20, %28                           45 << 1 = 90 (shl variable)
//!   %30 = add i16 %27, %29                           120 + 90 = 210
//! The plan's shape recomputed: clang strength-reduced `a % 7` to a
//! div-rem pair (urem uses divisor 5 instead), folded the constant `b`
//! (made runtime via `a - 320`), and widened `(c*7)/3` to i16 (the i8
//! mul/udiv come from explicit i8 casts through a volatile gate).

use std::collections::HashMap;
use std::process::Command;

fn muldiv_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/muldiv.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&m, &addrs);
    let asm = banking::assign_banks(&asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn muldiv_runs_correctly() {
    let layout = muldiv_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/muldiv.c", "tests/fixtures/muldiv.hex"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/muldiv.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    let val: u16 = 301; // in = 301 (little-endian i16)
    p.ram_mut()[in_addr] = (val & 0xFF) as u8;
    p.ram_mut()[in_addr + 1] = (val >> 8) as u8;
    p.run(500_000);
    let got = (p.ram()[out_addr] as u16) | ((p.ram()[out_addr + 1] as u16) << 8);
    assert_eq!(got, 210, "out == hand-computed 210 for in == 301");
    assert!(p.halted());
}
