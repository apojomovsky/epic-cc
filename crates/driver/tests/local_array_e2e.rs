//! Local-array alloca acceptance for #149: stack `[N x i8]` written and
//! read at a runtime index, the local counterpart to `array.c`. The
//! driver HEX must run to halt on the simulator with `out == 4` for
//! `in == 3` (buf[3] = 4, then out = buf[3]).

use std::process::Command;

/// `out`'s RAM address, read off the compiler's own `--map`
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
fn local_array_runs_correctly() {
    let hex_path = "tests/fixtures/local_array.hex";
    let map_path = "tests/fixtures/local_array.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/local_array.c",
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
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 3; // in low byte = 3 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[out_addr], 4, "out == buf[3] == 3+1");
    assert!(p.halted());
}
