//! A `const char *` walk over a flash string with no call argument (#1091).
//! The walk pointer is a phi over `.str` and its own gep, so each load
//! through it must read program memory (TBLRD). Before the fix the loads
//! went through INDF0 at the flash address and the final byte came back as
//! the first character ('E', 0x45) instead of the terminating '\n' (0x0a).
//!
//! The walk runs 26 times (`EPIC_HARNESS_RESULT: PASS\n`), so `sent` pins
//! the trip count as well as the last byte. The offset fixture reads through
//! a constant-offset gep over the walk phi (`q = p + 1; q[1]`), whose result
//! is pinned against the literal bytes.

use std::process::Command;

/// Compiles `fixture` for p18f4550 and runs it on the simulator. Returns the
/// RAM image, the `--map` text, and whether the program halted.
fn run_p18(fixture: &str) -> (Vec<u8>, String, bool) {
    let hex_path = std::env::temp_dir().join(format!(
        "flash_walk_{}_{}.hex",
        std::process::id(),
        std::path::Path::new(fixture)
            .file_stem()
            .unwrap()
            .to_string_lossy()
    ));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver p18f4550: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let _ = std::fs::remove_file(&hex_path);

    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    p.run(200_000);
    (p.ram().to_vec(), map, p.halted())
}

/// Reads a global's RAM address off the compiler's `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn flash_string_walk_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        0x0a,
        "last walked byte must be '\\n'"
    );
    assert_eq!(
        ram[map_addr(&map, "sent")],
        26,
        "walk must visit all 26 bytes"
    );
    assert!(halted);
}

#[test]
fn flash_string_walk_through_offset_gep_reads_program_memory_on_p18() {
    const S: &[u8] = b"EPIC_HARNESS_RESULT: PASS\n";
    // Iteration i loads `p[2]` for i in 0..25, so the sum covers S[2..26].
    let expected_sum = S[2..].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_offset.c");
    assert_eq!(
        ram[map_addr(&map, "sum")],
        expected_sum,
        "sum of q[1] over the walk"
    );
    assert_eq!(ram[map_addr(&map, "n")], 25, "walk must take 25 steps");
    assert!(halted);
}
