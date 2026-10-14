//! epic-cc#1276: clang emits `tail call void @llvm.assume(i1 %c)` for the
//! `__builtin_assume` guard, and legalize drops it (an assumption carries
//! no codegen). Acceptance: the driver's HEX runs to halt in the simulator
//! with `tail == 0` and `out == 3`.
//!
//! `out` and `tail`'s RAM addresses are read off the compiler's own `--map`
//! output. Outputs go to the temp dir so the tree stays clean.

use std::process::Command;

/// `name`'s RAM address, read off the compiler's own `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn assume_guard_compiles_and_runs() {
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("assume_{}.hex", std::process::id()));
    let map_path = dir.join(format!("assume_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/assume.c",
            "-o",
            &hex_path.display().to_string(),
            "--map",
            &map_path.display().to_string(),
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

    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "out");
    let tail_addr = map_addr(&map, "tail");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert_eq!(p.ram()[tail_addr], 0, "tail == buf[2] == 0");
    assert_eq!(p.ram()[out_addr], 3, "out == buf[0] + buf[1] == 3");
    assert!(p.halted());
}
