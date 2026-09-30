//! Optimization-profile matrix (epic-cc#839): the profile fixture must
//! build and behave identically under `-O0`/`-O1`/`-O2`/`-Os` on both
//! cores. Profiles change shaping (which calls fold, whether loop walks
//! are restructured, whether repeated runs are factored), never
//! semantics, so all four sim runs halt with the same checksum.

use std::process::Command;

/// The hand-computed checksum of `opt_profiles.c`: the folded byte walk
/// plus the three constant tail calls, all mod 256.
const EXPECTED: u8 = 176;

/// `checksum`'s RAM address, read off the compiler's own `--map` output
/// (`global {name} 0xNN` lines).
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Compile the fixture under one profile and return its HEX and map.
/// Outputs land in the temp dir under per-profile names, so the four
/// profiles never share a file.
fn compile(device: &str, profile: &str) -> (String, String) {
    let tag = format!("opt_profiles_{device}_{}", profile.trim_start_matches('-'));
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("{tag}.hex"));
    let map_path = dir.join(format!("{tag}.map"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--device", device, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path);
    if profile != "-Os" {
        cmd.arg(profile);
    }
    cmd.arg("tests/fixtures/opt_profiles.c");
    let out = cmd.output().expect("run driver");
    assert!(
        out.status.success(),
        "driver {device} {profile}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (hex, map)
}

fn checksum_pic14(hex: &str, map: &str, profile: &str) -> u8 {
    let addr = map_addr(map, "checksum");
    let prog = pic14_sim::parse_hex(hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(100_000);
    assert!(p.halted(), "p16f877a {profile} must halt");
    p.ram()[addr]
}

fn checksum_pic18(hex: &str, map: &str, profile: &str) -> u8 {
    let addr = map_addr(map, "checksum");
    let prog = pic14_sim::parse_hex_pic18(hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(100_000);
    assert!(p.halted(), "p18f4550 {profile} must halt");
    p.ram()[addr]
}

#[test]
fn every_profile_builds_and_agrees_on_pic14() {
    for profile in ["-O0", "-O1", "-O2", "-Os"] {
        let (hex, map) = compile("p16f877a", profile);
        assert_eq!(
            checksum_pic14(&hex, &map, profile),
            EXPECTED,
            "p16f877a {profile} checksum"
        );
    }
}

#[test]
fn every_profile_builds_and_agrees_on_pic18() {
    for profile in ["-O0", "-O1", "-O2", "-Os"] {
        let (hex, map) = compile("p18f4550", profile);
        assert_eq!(
            checksum_pic18(&hex, &map, profile),
            EXPECTED,
            "p18f4550 {profile} checksum"
        );
    }
}
