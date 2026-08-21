//! Milestone-9 multi-bank FSR acceptance: arrays pushed into banks 1-3 are
//! written and read through the FSR+IRP path at a runtime index, with a
//! banked direct copy, an sret call into a frame alloca, and a chained
//! dynamic index in the same program. Acceptance: (a) the emitted .asm uses
//! IRP (BSF/BCF STATUS, 7) on FSR setups — the M9 feature — including the
//! `& 0xFF` bank-2/3 base literal, and (b) the driver's HEX runs to halt in
//! the bank-aware simulator with `out == 0xB8` for `in == 3`.
//!
//! `in`, `out` and the arrays are globals; their addresses are read from the
//! same alloc layout the driver used (see the trace in fixtures/banked_ptr.c
//! for why the fixture looks the way it does).
//!
//! Hand computation from the emitted IR (in = 3, i = in & 3 = 3; the
//! volatile reloads are elided — only the value-bearing ops are traced):
//!   %2  = load volatile i16 @in                                  3
//!   %3  = and i16 %2, 3                                          3
//!   store volatile i8 0x11, arrB1[%3]                            arrB1[3] = 0x11
//!   store volatile i8 0x22, arrB2[%3]                            arrB2[3] = 0x22
//!   store volatile i8 0x33, arrB3[%3]                            arrB3[3] = 0x33
//!   out = arrB1[%3] + arrB2[%3] + arrB3[%3]                      0x11+0x22+0x33 = 0x66
//!   store volatile i8 0x07, arrB1[1]                             arrB1[1] = 0x07
//!   store volatile i8 arrB1[1], arrB2[5]                         arrB2[5] = 0x07 (BANKSEL copy)
//!   out = out + arrB2[5]                                         0x66 + 0x07 = 0x6D
//!   %16 = call sret %struct.P @mk()  -> g = {5, 6}               (sret into frame alloca)
//!   out = out + g.a + g.b                                        0x6D + 5 + 6 = 0x78
//!   store volatile i8 0x40, arrB3[arrB2[2]]                      arrB2[2] = 0 -> arrB3[0] = 0x40
//!   out = out + arrB3[0]                                         0x78 + 0x40 = 0xB8
//! The brief's draft used a constant `i = 3` (folded by clang -O1 into
//! constant GEPs, killing the FSR coverage) and copied arrB2[5] =
//! arrB1[1] *before* writing arrB1[1] = 0x07 (the copy would carry a stale
//! 0, making the final value 0xB1). Both adjusted while keeping the same
//! coverage (see fixtures/banked_ptr.c).

use std::collections::HashMap;
use std::process::Command;

/// Run clang + the full IR pipeline on the banked_ptr fixture, exactly as
/// the driver does, and return the alloc layout plus the final (banked) .asm.
fn banked_ptr_pipeline() -> (alloc::AllocLayout, String) {
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
            "tests/fixtures/banked_ptr.c",
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
    let asm = peephole::optimize(&asm);
    (layout, asm)
}

#[test]
fn banked_ptr_asm_uses_irp_across_banks() {
    let (_, asm) = banked_ptr_pipeline();
    // IRP = STATUS bit 7: the M9 FSR path must set it for bank-2/3 bases and
    // clear it for bank-0/1 bases on every FSR setup.
    assert!(
        asm.lines().any(|l| l.trim() == "BSF STATUS, 7"),
        "bank-2/3 FSR access must set IRP (BSF STATUS, 7):\n{asm}"
    );
    assert!(
        asm.lines().any(|l| l.trim() == "BCF STATUS, 7"),
        "bank-0/1 FSR access must clear IRP (BCF STATUS, 7):\n{asm}"
    );
    // arrB2's FSR base literal is 0x120 & 0xFF = 0x20 (the & 0xFF low-byte
    // base for a bank-2 object); without the M9 IRP path this setup would
    // not exist (a bank-0-only compiler would emit ADDLW 0x32).
    assert!(
        asm.lines().any(|l| l.trim() == "ADDLW 0x20"),
        "bank-2 FSR base must be the & 0xFF literal 0x20 (0x120 & 0xFF):\n{asm}"
    );
}

#[test]
fn banked_ptr_runs_correctly() {
    let (layout, _) = banked_ptr_pipeline();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;
    let in_addr = *layout.globals.get("in").expect("in global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/banked_ptr.c", "-o", "tests/fixtures/banked_ptr.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/banked_ptr.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 3; // in low byte = 3 (high byte stays 0)
    p.run(2_000_000);
    assert_eq!(p.ram()[out_addr], 0xB8, "out == hand-computed 0xB8 for in == 3");
    assert!(p.halted());
}
