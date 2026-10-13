//! Packed static flags end to end (epic-cc#1162): the fixture's two
//! flags narrow to `global i1`, share one RAM byte, and lower to bit
//! ops. The test proves packing happened (both names at one map
//! address) and drives every bit pattern through the simulator: `in`
//! is poked per pattern and `out` must echo it back, so a wrong bit,
//! a disturbed neighbor, or a clear that never landed fails the run.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    map.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("global ")
                .and_then(|r| r.split_once(' '))
                .filter(|(n, _)| *n == name)
                .and_then(|(_, a)| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
        })
        .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
}

#[test]
fn packed_flags_share_a_byte_and_round_trip() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/packed_flags.c",
            "-o",
            "tests/fixtures/packed_flags_18F4550.hex",
            "--map",
            "tests/fixtures/packed_flags_18F4550.map",
            "--device",
            "18F4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = std::fs::read_to_string("tests/fixtures/packed_flags_18F4550.map").unwrap();
    let (fa, fb) = (map_addr(&map, "f_a"), map_addr(&map, "f_b"));
    assert_eq!(fa, fb, "the two flags must share one byte:\n{map}");
    let (ia, oa) = (map_addr(&map, "in"), map_addr(&map, "out"));
    let hex = std::fs::read_to_string("tests/fixtures/packed_flags_18F4550.hex").unwrap();
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    // main loops forever, so no halt: drive each pattern, let one full
    // set/read iteration settle, then read `out` back.
    for pattern in [0u8, 1, 2, 3] {
        p.ram_mut()[ia] = pattern;
        for _ in 0..2000 {
            assert!(!p.halted(), "main must loop, not halt");
            p.step();
        }
        assert_eq!(
            p.ram()[oa],
            pattern,
            "pattern {pattern:#04x} did not round-trip"
        );
    }
    let _ = std::fs::remove_file("tests/fixtures/packed_flags_18F4550.hex");
    let _ = std::fs::remove_file("tests/fixtures/packed_flags_18F4550.map");
}
