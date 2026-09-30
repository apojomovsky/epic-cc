//! Milestone-6 scalar acceptance: an ordinary embedded-C program whose loop
//! exercises the newly supported scalar surface — `sub`, `and i8`, `or`,
//! `xor`, and the `eq`/`ne`/`ugt`/`ult` icmp predicates — compiles through
//! the whole driver pipeline and runs correctly in the simulator. Acceptance:
//! for `in == 7` the hand-computed `out == 174` and the machine halts.
//!
//! `in` and `out` are i8 globals; their addresses are read from the
//! compiler's own `--map` output (see the trace in fixtures/scalar.c).

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
fn scalar_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/scalar.c",
            "-o",
            "tests/fixtures/scalar.hex",
            "--map",
            "tests/fixtures/scalar.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/scalar.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/scalar.map").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/scalar.map");
    let in_addr = map_addr(&map, "in");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[in_addr] = 7; // in = 7
    p.run(200_000);
    assert_eq!(
        p.ram()[out_addr],
        174,
        "out == hand-computed 174 for in == 7"
    );
    assert!(p.halted());
}
