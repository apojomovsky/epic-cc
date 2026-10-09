//! Divmod edge coverage (epic-cc#894): the fixture self-checks defined
//! udiv/urem/sdiv/srem cases across widths and records poison cases
//! (divisor zero, INT_MIN/-1) raw. Both profiles must halt with zero
//! failures, and the poison records must agree across profiles: the `-O2`
//! loop-tail fold may reshape the loop, never its trajectory.

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
    let tag = format!("divmod_edge_{}", profile.trim_start_matches('-'));
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("{tag}.hex"));
    let map_path = dir.join(format!("{tag}.map"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path);
    if profile != "-Os" {
        cmd.arg(profile);
    }
    cmd.arg("tests/fixtures/divmod_edge.c");
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

fn run(hex: &str) -> Vec<u8> {
    let prog = pic14_sim::parse_hex_pic18(hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(2_000_000);
    assert!(p.halted(), "divmod_edge must halt");
    p.ram().to_vec()
}

#[test]
fn divmod_edge_profiles_agree() {
    let (hex_os, map_os) = compile("-Os");
    let (hex_o2, map_o2) = compile("-O2");
    let ram_os = run(&hex_os);
    let ram_o2 = run(&hex_o2);
    for (map, ram, profile) in [(&map_os, &ram_os, "-Os"), (&map_o2, &ram_o2, "-O2")] {
        let fails = map_addr(map, "fails");
        assert_eq!(ram[fails], 0, "{profile}: no defined case may fail");
    }
    // Poison records agree byte for byte across profiles.
    for (name, bytes) in [
        ("p8q", 1),
        ("p8m", 1),
        ("p16q", 2),
        ("p16m", 2),
        ("p32q", 4),
        ("p32m", 4),
        ("ps16q", 2),
        ("ps16m", 2),
        ("ps32q", 4),
        ("ps32m", 4),
    ] {
        let a_os = map_addr(&map_os, name);
        let a_o2 = map_addr(&map_o2, name);
        assert_eq!(
            ram_os[a_os..a_os + bytes],
            ram_o2[a_o2..a_o2 + bytes],
            "poison record {name} agrees across profiles"
        );
    }
}
