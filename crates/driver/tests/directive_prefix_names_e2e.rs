//! Acceptance for function names that begin with assembler directive names
//! (`endpoint`, `listen`, `radix`). Before the fix the driver aborted with an
//! internal error: the bare `end`/`list`/`radix` prefix tests dropped those
//! labels from the listing, so their flash placement lookup found nothing.
//! Acceptance: the program compiles and runs to `out == 1 + 2 + 4`.
//!
//! `out`'s RAM address is read off the compiler's own `--map` output.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn directive_prefixed_function_names_run_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/directive_prefix_names.c",
            "-o",
            "tests/fixtures/directive_prefix_names.hex",
            "--map",
            "tests/fixtures/directive_prefix_names.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/directive_prefix_names.hex").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/directive_prefix_names.hex");
    let map =
        std::fs::read_to_string("tests/fixtures/directive_prefix_names.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/directive_prefix_names.map");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 7, "endpoint 1 + listen 2 + radix 4");
    assert!(p.halted());
}
