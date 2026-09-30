//! CC-6 reporting acceptance: the size report on stderr matches the HEX and
//! the allocator's layout, and --map writes the allocator's map text.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("epic-cc-size-map-{}-{name}", std::process::id()));
    p
}

fn fixture_add() -> String {
    format!("{}/tests/fixtures/add.c", env!("CARGO_MANIFEST_DIR"))
}

/// One alloc scalar off the compiler's own `--map` output: the hex after
/// `total-bank0 0x`, `bank-used <i> 0x`, and friends. Rebuilding the
/// pipeline here instead would be a second copy of `main.rs` that
/// silently drifts (see array_e2e's `map_addr`).
fn map_scalar(map: &str, kind: &str) -> u16 {
    let prefix = format!("{kind} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {kind} in:\n{map}"));
    u16::from_str_radix(line[prefix.len()..].trim(), 16).expect("map scalar is hex")
}

#[test]
fn size_report_matches_hex_and_layout() {
    let hex_path = tmp("add.hex");
    let map_path = tmp("report.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "--map",
            map_path.to_str().unwrap(),
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flash: count the program words in the HEX (the highest nonzero word
    // address + 1, the same trim to_hex applies).
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let flash_used = prog
        .iter()
        .rposition(|&w| w != 0)
        .map(|i| i + 1)
        .unwrap_or(0);

    let map = std::fs::read_to_string(&map_path).expect("read map");
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(
        report.contains(&format!("flash: {flash_used}/8192 words")),
        "flash line missing or wrong: {report}"
    );
    // RAM: the report's bank lines must match the map's bank-used scalars
    // and the device's bank sizes.
    for (i, &(start, end)) in device::PIC16F877A.ram_banks.iter().enumerate() {
        let total = end - start + 1;
        let used = map_scalar(&map, &format!("bank-used {i}"));
        assert!(
            report.contains(&format!("bank {i}: {used}/{total} bytes")),
            "bank {i} line missing or wrong: {report}"
        );
    }
    // The report states what it means by used.
    assert!(
        report.contains("overlay"),
        "RAM line must state the overlay definition: {report}"
    );
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
}

#[test]
fn map_file_matches_the_allocator_map() {
    let hex_path = tmp("map.hex");
    let map_path = tmp("add.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "--map",
            map_path.to_str().unwrap(),
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(&map_path).unwrap();
    // The map is the allocator's own addresses, in the driver's HashMap
    // key form ({func}::{name} locals), sorted deterministically, then
    // the alloc scalars (epic-cc#814). add.c has two globals and two i8
    // locals and no ISR, so the exact lines are pinned.
    assert_eq!(
        written,
        "; epic-cc map for p16f877a\n\
         global in 0x20\n\
         global out 0x21\n\
         local main::1 0x22\n\
         local main::2 0x23\n\
         total-bank0 0x02\n\
         bank-used 0 0x04\n\
         bank-used 1 0x00\n\
         bank-used 2 0x00\n\
         bank-used 3 0x00\n"
    );
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
}
