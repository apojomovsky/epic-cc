//! Pooled member reads at a constant offset (epic-cc#913): with
//! `--const-pool` a gated member is read through its pool chunk's reader at
//! the member's offset, with no per-const table of its own. The simulator
//! must return the same bytes as the unpooled build.

use std::process::Command;

/// RAM address of a global, read off the compiler's own `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Compiles the fixture with `extra` flags, simulates it to halt, and
/// returns the three output bytes.
fn run(tag: &str, extra: &[&str]) -> [u8; 3] {
    // Unique temp paths: the suite runs tests in parallel.
    let dir = std::env::temp_dir();
    let stem = format!("pool_gated_{tag}_{}", std::process::id());
    let hex_path = dir.join(format!("{stem}.hex"));
    let map_path = dir.join(format!("{stem}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg("tests/fixtures/pool_gated_reads.c")
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p16f877a"])
        .args(extra)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {tag}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);

    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(200_000);
    assert!(p.halted(), "{tag} must halt");
    let ram = p.ram();
    [
        ram[map_addr(&map, "g_out0")],
        ram[map_addr(&map, "g_out1")],
        ram[map_addr(&map, "g_out2")],
    ]
}

#[test]
fn pooled_member_reads_match_unpooled_build() {
    // 30 + 60 from memcpy(&tbl[2]); 'l' = txt[1 + 2]; strlen("hello-world").
    let expected = [90, 108, 11];
    assert_eq!(run("base", &[]), expected, "unpooled build");
    assert_eq!(
        run("pool", &["--const-pool"]),
        expected,
        "--const-pool build"
    );
}
