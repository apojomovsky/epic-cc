//! Issue #3 acceptance: const (flash) lookup tables of multi-byte elements
//! (i16 / i32 / float). clang -O1 emits such tables as typed element lists
//! (`[130 x i16] [i16 4096, ...]`, `[100 x i32] [i32 ...]`,
//! `[100 x float] [float 0x3FB99999A0000000, ...]`) — never `c"..."` — so
//! the pipeline must (a) decode element lists into little-endian table
//! bytes (irparse), and (b) read multi-byte elements at runtime (isel):
//! scale-2/scale-4 GEP terms through the small-table accumulator path and
//! the chunked 16-bit large-table readers, with the element scale applied
//! to the index (RLF chains — classic mid-range has no MULLW).
//!
//! Hand computation for in == 290 (0x0122), against the fixture's element
//! patterns (see fixtures/const_multi_byte.c; every read uses a runtime
//! index so clang cannot fold it to a literal):
//!   out_s16 = t16s[290 & 3 = 2]       = 0x9ABC
//!   out_s32 = t32s[2]                 = 0x090A0B0C
//!   out_l16 = t16[290 & 0x7F = 34]    = 0x1022  (byte 68,  chunk 0)
//!   out_l16b= t16[128 + (290&1 = 0)]  = 0x1080  (byte 256, chunk 1 — scale-2 carry)
//!   out_l32 = t32[290 & 0x3F = 34]    = 0x23242526 (byte 136, chunk 0)
//!   out_l32b= t32[64 + 0]             = 0x41424344 (byte 256, chunk 1 — scale-4 carry)
//!   outf    = tf[34]                  = 3.4f = 0x4059999A (f64-narrowed init)
//!   outf2   = tf[64]                  = 6.4f = 0x40CCCCCD
//!
//! The small tables t16s (6 bytes) / t32s (12 bytes) exercise scale-2/4
//! GEP terms on the non-chunked path (accumulator); t16 (260 bytes), t32
//! and tf (400 each) exercise the chunked large-table readers with scaled
//! indices, including the exact boundary where byte 256 (chunk 1) is
//! selected by the scale's carry.

use std::collections::HashMap;
use std::process::Command;

fn multi_byte_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/const_multi_byte.c",
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

fn read_le4(ram: &[u8], addr: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..4 {
        v |= (ram[addr + i] as u32) << (8 * i);
    }
    v
}

#[test]
fn multi_byte_const_tables_run_correctly() {
    let layout = multi_byte_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let a = |n: &str| *layout.globals.get(n).expect(n) as usize;
    let out_s16 = a("out_s16");
    let out_s32 = a("out_s32");
    let out_l16 = a("out_l16");
    let out_l16b = a("out_l16b");
    let out_l32 = a("out_l32");
    let out_l32b = a("out_l32b");
    let outf = a("outf");
    let outf2 = a("outf2");

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/const_multi_byte.c", "-o", "tests/fixtures/const_multi_byte.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/const_multi_byte.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 0x22; // in = 290
    p.ram_mut()[in_addr + 1] = 0x01;
    p.run(5_000_000);

    assert_eq!(read_le4(p.ram(), out_s16) & 0xFFFF, 0x9ABC, "out_s16 == 0x9ABC (small i16 table)");
    assert_eq!(read_le4(p.ram(), out_s32), 0x090A0B0C, "out_s32 == 0x090A0B0C (small i32 table)");
    assert_eq!(read_le4(p.ram(), out_l16) & 0xFFFF, 0x1022, "out_l16 == 0x1022 (i16 chunk 0)");
    assert_eq!(read_le4(p.ram(), out_l16b) & 0xFFFF, 0x1080, "out_l16b == 0x1080 (i16 chunk 1, scale-2 carry)");
    assert_eq!(read_le4(p.ram(), out_l32), 0x23242526, "out_l32 == 0x23242526 (i32 chunk 0)");
    assert_eq!(read_le4(p.ram(), out_l32b), 0x41424344, "out_l32b == 0x41424344 (i32 chunk 1, scale-4 carry)");
    assert_eq!(read_le4(p.ram(), outf), 0x4059999A, "outf == 3.4f (float chunk 0, f64-narrowed init)");
    assert_eq!(read_le4(p.ram(), outf2), 0x40CCCCCD, "outf2 == 6.4f (float chunk 1)");
    assert!(p.halted());
}
