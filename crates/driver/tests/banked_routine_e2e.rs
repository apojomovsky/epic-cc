//! Issue #6 acceptance: a runtime routine whose whole frame lands in a
//! non-zero GPR bank compiles through the whole driver pipeline and runs
//! correctly.
//!
//! Layout (see fixtures/banked_routine.c): `out` u16 at 0x20, `g[30]` (60
//! bytes) at 0x22..0x5D, so the root frame starts at 0x5E. The noinline
//! helper `mul30` carries ~120 bytes of i16 SSA defs (volatile loads +
//! adds), pushing its physical frame end well past the bank-0/1 gap;
//! `__mul_u16`'s frame is derived at that end, so it sits entirely inside
//! a single non-zero bank. Before issue #6 the isel assert rejected any
//! routine slot > 0x7F with a loud panic.
//!
//! Acceptance: (a) the routine frame lies wholly inside one GPR bank, and
//! (b) the driver's HEX runs to halt with `out == 0x0CB7` (sum 1..30 =
//! 465, 465 * 7 = 3255).

use std::collections::HashMap;
use std::process::Command;

use device::PIC16F877A;

/// Run clang + the full IR pipeline on the fixture, exactly as the driver
/// does, and return the alloc layout plus the final (banked) .asm.
fn banked_routine_pipeline() -> (alloc::AllocLayout, String) {
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
            "tests/fixtures/banked_routine.c",
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
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
    let asm = peephole::optimize(&asm);
    (layout, asm)
}

#[test]
fn routine_frame_lands_in_bank1() {
    let (layout, _) = banked_routine_pipeline();
    let scr = *layout
        .locals
        .get("__mul_u16::__scr")
        .expect("__mul_u16::__scr");
    let a = *layout.locals.get("__mul_u16::a").expect("__mul_u16::a");
    let b = *layout.locals.get("__mul_u16::b").expect("__mul_u16::b");
    // The whole frame (params + 14-byte scratch) must sit inside ONE bank,
    // any bank (issue #6): a straddle would need a BANKSEL inside the
    // skip-sensitive recipe loops. The helper's 120-byte frame pushes this
    // one to bank 1 or 2 depending on clang's exact def count.
    assert!(
        scr >= 0x20,
        "routine frame must live in a GPR bank (got __scr at 0x{scr:03X})"
    );
    let all = [a, b, scr, scr + 13];
    let bank = PIC16F877A
        .ram_banks
        .iter()
        .position(|&(s, e)| a >= s && a <= e);
    let bank = bank.expect("routine params must be inside a GPR bank");
    let (bs, be) = PIC16F877A.ram_banks[bank];
    for &x in &all {
        assert!(
            x >= bs && x <= be,
            "frame byte 0x{x:03X} is outside bank {bank} (0x{bs:03X}-0x{be:03X}); \
             the recipe loops are skip-sensitive and a BANKSEL would change skip targets"
        );
    }
}

#[test]
fn banked_routine_runs_correctly() {
    let (layout, _) = banked_routine_pipeline();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/banked_routine.c",
            "-o",
            "tests/fixtures/banked_routine.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/banked_routine.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(2_000_000);
    assert_eq!(
        p.ram()[out_addr],
        0xB7,
        "out low byte: 3255 = 0x0CB7 (little-endian)"
    );
    assert_eq!(p.ram()[out_addr + 1], 0x0C, "out high byte: 3255 = 0x0CB7");
    assert!(p.halted());
}
