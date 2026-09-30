//! epic-cc#160 regression: llvm.umin and llvm.usub.sat lower correctly.
//! clang -O1 emits llvm.umin for the priority min-finding loop and
//! llvm.usub.sat for the guarded decrement of the memory value; legalize
//! now lowers both (icmp ult + select, icmp uge + sub + select).

use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn umin_and_usub_sat_run_correctly() {
    let hex_path = std::env::temp_dir().join(format!("umin_usub_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/umin_usub.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let prog = pic14_sim::parse_hex(&produced);
    let mut sim = pic14_sim::Pic14::new(prog);
    sim.run(100_000);
    let min_addr = map_addr(&map, "g_min");
    let count_addr = map_addr(&map, "g_count");
    assert_eq!(sim.ram()[min_addr], 0, "min of 3,1,2,0 == 0");
    assert_eq!(sim.ram()[count_addr], 1, "guarded decrement of 2 == 1");
}
