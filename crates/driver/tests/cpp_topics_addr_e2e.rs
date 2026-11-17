//! epic-cc#1478 acceptance: a C++ internal-linkage const table that is
//! both a plain call argument and stored into a RAM pointer must keep a
//! RAM copy on PIC14E, instead of staging into the shared buffer and
//! aborting with `isel: no address for @_ZL8topics_a`.

use std::process::Command;

#[test]
fn cpp_topics_table_stored_as_value_compiles_on_pic14e() {
    let tag = std::process::id();
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("cpp_topics_addr_{tag}.hex"));
    let map_path = dir.join(format!("cpp_topics_addr_{tag}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cpp_topics_addr.cpp",
            "tests/fixtures/cpp_topics_addr_c.c",
            "--target",
            "p16f1938",
            "-o",
        ])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    // The stored address must name real RAM: the map carries the table
    // as an allocated global, never as a staged const.
    assert!(
        map.lines().any(|l| l.starts_with("global _ZL8topics_a 0x")),
        "topics_a must have a RAM address in the map:\n{map}"
    );
    assert!(
        !map.lines().any(|l| l == "staged _ZL8topics_a"),
        "topics_a must not be staged:\n{map}"
    );
}
