//! A callee that stores the address of a caller's local buffer keeps
//! reading the bytes after the call, so the buffer's frame slot must not
//! be reused by a value defined after the call (epic-cc#1147). The driver
//! HEX must run to halt on the simulator with `out == in` (the buffer byte
//! read back through the stored pointer) and `out2 == (in + 1) ^ 0x5A`.

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
fn retained_local_buffer_reads_back_after_later_values() {
    let dir = std::env::temp_dir().join(format!("epic-cc-alloca-retention-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hex_path = dir.join("alloca_retention.hex");
    let map_path = dir.join("alloca_retention.map");

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/alloca_retention.c", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p16f877a"])
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
    let (in_addr, out_addr, out2_addr) = (
        map_addr(&map, "in"),
        map_addr(&map, "out"),
        map_addr(&map, "out2"),
    );
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.ram_mut()[in_addr] = 0x11;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[out2_addr], (0x11u8 + 1) ^ 0x5A, "z");
    assert_eq!(p.ram()[out_addr], 0x11, "tab[0] must read buf[0]");
}
