//! Milestone-10 const-table acceptance: a 300-byte const (flash) table read
//! through the two-entry chunked readers (`__read_table` / `__read_table_hi`
//! with `table` / `table_1` chunk labels), landing at 0x100 (window 1) past
//! a 40-byte `pad` filler so the readers' `MOVLW HIGH(...); MOVWF PCLATH`
//! sets are load-bearing — without them the computed `ADDLW LOW(...);
//! MOVWF PCL` jumps would land in window 0 and every read would return a
//! wrong byte.
//!
//! Hand computation for in == 290 (0x0122), against the exact emitted IR
//! (clang -O1 folds none of the four reads — all indices are runtime; a
//! literal `table[299]`/`table[256]` would be folded to constants, so the
//! fixture uses `table[in + 9]`/`table[in - 34]` to keep the reads real):
//!   - out = table[290]: lo 0x22 = 34, hi 1 -> chunk 1, in-chunk 34
//!     -> 0x11 + 34 = 0x33
//!   - out += table[290 & 3] = table[2] = 0x02 (chunk 0)
//!   - out += table[290 + 9] = table[299]: in-chunk 43 -> 0x11 + 43 = 0x3C
//!   - out += table[290 - 34] = table[256]: lo 0x00, hi 1 -> chunk-1 first
//!     byte -> 0x11
//!   - out = (0x33 + 0x02 + 0x3C + 0x11) & 0xFF = 0x82
//!
//! `out`'s address is read off the compiler's own `--map` output (in
//! is the i16 global at 0x20-0x21; out follows at 0x22).

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

#[test]
fn const_table_reads_past_256_bytes_run_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_table.c",
            "-o",
            "tests/fixtures/const_table.hex",
            "--map",
            "tests/fixtures/const_table.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/const_table.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/const_table.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/const_table.map");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(200_000);
    assert_eq!(
        p.ram()[out_addr],
        0x82,
        "out == 0x82 for in == 290 (chunk-1, chunk-0, chunk-1-last, boundary reads)"
    );
    assert!(p.halted());
}
