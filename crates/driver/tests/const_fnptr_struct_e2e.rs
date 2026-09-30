//! epic-cc#154 acceptance: a `static const` struct with a function-pointer
//! field (a table-driven FSM's transition rows) must decode and dispatch.
//!
//! The e2e compiles the fixture, runs it in the sim with `g_idx` set to
//! each row, and asserts the guard dispatch: row 0's guard is non-null and
//! returns 1 (g_count < 2), row 1's guard is null and out stays 0.
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
fn const_fnptr_struct_dispatches_the_guard() {
    let hex_path = "tests/fixtures/const_fnptr_struct.hex";
    let map_path = "tests/fixtures/const_fnptr_struct.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_fnptr_struct.c",
            "-o",
            hex_path,
            "--map",
            map_path,
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
    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(map_path);
    let idx_addr = map_addr(&map, "g_idx");
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);

    // Row 0: guard non-null -> out = guard(0) = 1.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[idx_addr] = 0;
    p.run(200_000);
    assert!(p.halted(), "row 0 must halt");
    assert_eq!(p.ram()[out_addr], 1, "row 0 guard dispatched");

    // Row 1: guard null -> out stays 0.
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[idx_addr] = 1;
    p.run(200_000);
    assert!(p.halted(), "row 1 must halt");
    assert_eq!(p.ram()[out_addr], 0, "row 1 guard is null");
    let _ = std::fs::remove_file(hex_path);
}
