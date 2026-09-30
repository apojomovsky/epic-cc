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

use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
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
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_3chunk.c",
            "-o",
            "tests/fixtures/const_3chunk.hex",
            "--map",
            "tests/fixtures/const_3chunk.map",
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
    let map = std::fs::read_to_string("tests/fixtures/const_3chunk.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/const_3chunk.map");
    let in_addr = map_addr(&map, "in");
    let a = |n: &str| map_addr(&map, n);
    let out8 = a("out8");
    let o16_0 = a("o16_0");
    let o16_1 = a("o16_1");
    let o16_2 = a("o16_2");

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
