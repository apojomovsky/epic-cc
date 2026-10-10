//! Rem idiom coverage (epic-cc#1008): non-volatile `%` reaches the divmod
//! fusion as clang's `x - (x / y) * y`. The fixture self-checks the identities
//! on the PIC14 simulator under both profiles, and each build must carry the
//! fused routine's remainder slot, which exists only when the fusion fired.

use std::process::Command;

/// `global {name} 0xNN` address lookup off the compiler's own `--map`.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn compile(profile: &str) -> (String, String) {
    let tag = format!("divmod_rem_idiom_{}", profile.trim_start_matches('-'));
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("{tag}.hex"));
    let map_path = dir.join(format!("{tag}.map"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--device", "p16f877a", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path);
    if profile != "-Os" {
        cmd.arg(profile);
    }
    cmd.arg("tests/fixtures/divmod_rem_idiom.c");
    let out = cmd.output().expect("run driver");
    assert!(
        out.status.success(),
        "driver {profile}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (hex, map)
}

#[test]
fn rem_idiom_fuses_and_self_checks() {
    for profile in ["-Os", "-O2"] {
        let (hex, map) = compile(profile);
        map_addr(&map, "__udivmod_rem_u16");
        map_addr(&map, "__sdivmod_rem_i16");
        let fails = map_addr(&map, "fails");
        let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
        p.run(20_000_000);
        assert!(p.halted(), "{profile}: rem idiom fixture must halt");
        assert_eq!(p.ram()[fails], 0, "{profile}: no identity may fail");
    }
}
