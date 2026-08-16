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
//! `out`'s address is read from the same alloc layout the driver used (in
//! is the i16 global at 0x20-0x21; out follows at 0x22).

use std::collections::HashMap;
use std::process::Command;

fn const_table_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/const_table.c",
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
    let layout = alloc::allocate(&m, &callgraph::edges_text(&cg));
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&m, &addrs);
    let asm = banking::assign_banks(&asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn const_table_reads_past_256_bytes_run_correctly() {
    let layout = const_table_layout();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/const_table.c", "tests/fixtures/const_table.hex"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/const_table.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 0x82, "out == 0x82 for in == 290 (chunk-1, chunk-0, chunk-1-last, boundary reads)");
    assert!(p.halted());
}
