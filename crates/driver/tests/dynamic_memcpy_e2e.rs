//! Issue #4 acceptance: runtime-length memcpy. Two `llvm.memcpy` calls with
//! i16 register lengths (`i16 %4`, `i16 %5`) — the counted-loop path — plus
//! the loop's zero-length guard and the constant-index reads after the copy.
//!
//! Hand computation for in == 0x0A (10):
//!   k  = in & 0xFF = 10      -> memcpy(buf1, buf2, 10): buf1[i] = buf2[i]
//!   k2 = (in >> 4) = 0       -> memcpy(buf3, buf2, 0): the guard skips the
//!                               loop, buf3 stays all zeros
//!   out = buf1[9] + buf3[4]  = (9*0x37 & 0xFF) + 0x00 = 0xEF + 0x00 = 0xEF
//!
//! The 16-byte spans of buf1/buf2/buf3 fit one FSR window (0x20..0x80), so
//! the per-byte FSR recomputes stay window-legal at every index.

use std::collections::HashMap;
use std::process::Command;

fn dyn_memcpy_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/dynamic_memcpy.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

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
fn dynamic_length_memcpy_runs_correctly() {
    let layout = dyn_memcpy_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;
    let buf1 = *layout.globals.get("buf1").expect("buf1 global") as usize;
    let buf3 = *layout.globals.get("buf3").expect("buf3 global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/dynamic_memcpy.c",
            "tests/fixtures/dynamic_memcpy.hex",
        ])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/dynamic_memcpy.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    // RAM globals are not initialized by the pipeline (the simulator starts
    // zeroed, like float_e2e's `in`); seed buf2's pattern — the copy source.
    let buf2 = *layout.globals.get("buf2").expect("buf2 global") as usize;
    for i in 0..16 {
        p.ram_mut()[buf2 + i] = (i as u8).wrapping_mul(0x37);
    }
    p.ram_mut()[in_addr] = 0x0A; // in = 10
    p.ram_mut()[in_addr + 1] = 0x00;
    p.run(2_000_000);

    assert_eq!(p.ram()[out_addr], 0xEF, "out == 0xEF for in == 10 (10-byte runtime copy + zero-length guard)");
    // buf1[0..9] = the pattern (10 bytes copied)
    for i in 0..10 {
        assert_eq!(p.ram()[buf1 + i], (i as u8).wrapping_mul(0x37), "buf1[{i}]");
    }
    assert_eq!(p.ram()[buf1 + 10], 0, "buf1[10] untouched (only 10 bytes copied)");
    // buf3 untouched by the zero-length copy
    for i in 0..8 {
        assert_eq!(p.ram()[buf3 + i], 0, "buf3[{i}] stays zero (len-0 guard)");
    }
    assert!(p.halted());
}
