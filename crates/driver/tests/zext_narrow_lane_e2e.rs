//! banked_ptr.c plus a call that leaves a nonzero byte where the final
//! chained index temp's high lane lives (epic-cc#1237). A lane-0-only zext
//! feeding a full-width FSR index add must not read that stale byte: the
//! hand trace of banked_ptr.c still gives `out == 0xB8` for `in == 3`.

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
fn stale_high_byte_does_not_reach_the_index_add() {
    let dir = std::env::temp_dir().join(format!("epic-cc-zext-lane-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hex_path = dir.join("banked_ptr_stale_hi.hex");
    let map_path = dir.join("banked_ptr_stale_hi.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/banked_ptr_stale_hi.c", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p18f4550"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    p.ram_mut()[map_addr(&map, "in")] = 3;
    p.run(2_000_000);
    assert_eq!(p.ram()[map_addr(&map, "out")], 0xB8);
}
