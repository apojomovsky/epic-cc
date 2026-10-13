//! epic-cc#1223 acceptance: the epic-hal epic-bus shape on
//! PIC18. A `static const` ops table of function pointers is installed
//! through a RAM pointer (a store of a const address, #1223) and the mem
//! transactions dispatch every field of the table through that pointer
//! (the multi-byte FSR0 walk, #1221). `g_out` is 1 only when every call
//! reached its mock, in order, with the loopback data intact.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn run_fixture(fixture: &str, tag: &str) -> u8 {
    let hex_path = std::env::temp_dir().join(format!("{tag}_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "g_out");
    let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    sim.run(200_000);
    sim.ram()[out_addr]
}

#[test]
fn ops_table_dispatches_every_field_through_a_ram_pointer() {
    assert_eq!(
        run_fixture("tests/fixtures/bus_ops_dispatch.c", "bus_ops"),
        1
    );
}
