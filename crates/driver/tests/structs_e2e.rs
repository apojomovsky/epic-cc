//! Milestone-7 structs acceptance: a program exercising the full M7 surface
//! — sret calls (`mk`), byval calls from globals (`sum`, `pick`), struct
//! copies (`g = mk(...)` -> volatile memcpy), dynamic array-in-struct
//! indexing (`arr.v[arr.n]`, `x.v[x.n]`), and nested-struct field math with
//! folded byte GEPs (`go.in.a / go.in.b / go.z` at offsets 0/2/4) — compiles
//! through the whole driver pipeline and runs correctly in the simulator.
//! Acceptance: `out == 0x4E` (hand-computed, see the trace in
//! fixtures/structs.c) and the machine halts.
//!
//! `out` is the i8 global at the address the compiler's own `--map`
//! output gives it.

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
fn structs_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/structs.c",
            "-o",
            "tests/fixtures/structs.hex",
            "--map",
            "tests/fixtures/structs.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/structs.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/structs.map").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/structs.map");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 0x4E, "out == hand-computed 0x4E");
    assert!(p.halted());
}
