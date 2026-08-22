//! Issue #8 acceptance: const (flash) tables past the 511-byte two-chunk
//! bound. clang -O1 emits a 600-byte `const unsigned char[600]` as a
//! `c"..."` literal and a 300-element i16 table as a typed element list
//! (issue #3's form); the pipeline must (a) accept tables up to the 16-bit
//! index space (65535 bytes) instead of panicking at 511 (irparse), (b)
//! emit three-or-more 256-byte chunks with a reader entry per chunk (isel),
//! (c) select the chunk from the full 16-bit index — the descending
//! `scratch >= c` chain — and (d) scale multi-byte indices through the hi
//! byte (element 256 of t16b is byte 512 = chunk 2, not chunk 1: the
//! old 2-chunk code ignored idx_hi and would have read byte 256's value).
//!
//! Hand computation for in == 290 (0x0122), against the fixture's element
//! patterns (see fixtures/const_3chunk.c; every read uses a runtime index
//! so clang cannot fold it to a literal):
//!   out8 = t600[290 & 0x7F = 34]    = 0xF5  chunk 0
//!        + t600[128 + (290&1 = 0)]  = 0x8B  chunk 0 (128 = 0x80 term)
//!        + t600[256 + 0]            = 0x0B  chunk 1 FIRST byte
//!        + t600[512 + (290&7 = 2)]  = 0x55  chunk 2 first region
//!        + t600[599 - 2]           = 0x54  chunk 2 last byte region
//!        sum & 0xFF = (245+139+11+85+84) & 0xFF = 0x34
//!   o16_0 = t16b[34]   = 0x1022 (byte 68,  chunk 0)
//!   o16_1 = t16b[128]  = 0x1080 (byte 256, chunk 1 — scale-2 carry)
//!   o16_2 = t16b[256]  = 0x1100 (byte 512, chunk 2 — scale-2 hi-byte carry!)

use std::collections::HashMap;
use std::process::Command;

fn three_chunk_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/const_3chunk.c"),
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

fn read_le4(ram: &[u8], addr: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..4 {
        v |= (ram[addr + i] as u32) << (8 * i);
    }
    v
}

#[test]
fn three_chunk_const_tables_run_correctly() {
    let layout = three_chunk_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let a = |n: &str| *layout.globals.get(n).expect(n) as usize;
    let out8 = a("out8");
    let o16_0 = a("o16_0");
    let o16_1 = a("o16_1");
    let o16_2 = a("o16_2");

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_3chunk.c",
            "-o",
            "tests/fixtures/const_3chunk.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/const_3chunk.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 0x22; // in = 290
    p.ram_mut()[in_addr + 1] = 0x01;
    p.run(5_000_000);

    assert_eq!(
        p.ram()[out8],
        0x34,
        "out8 == 0x34 for in == 290 (chunk-0, chunk-1-first, chunk-2 reads)"
    );
    assert_eq!(
        read_le4(p.ram(), o16_0) & 0xFFFF,
        0x1022,
        "o16_0 == 0x1022 (i16 chunk 0)"
    );
    assert_eq!(
        read_le4(p.ram(), o16_1) & 0xFFFF,
        0x1080,
        "o16_1 == 0x1080 (i16 chunk 1, scale-2 carry)"
    );
    assert_eq!(
        read_le4(p.ram(), o16_2) & 0xFFFF,
        0x1100,
        "o16_2 == 0x1100 (i16 chunk 2, scale-2 hi-byte carry)"
    );
    assert!(p.halted());
}
