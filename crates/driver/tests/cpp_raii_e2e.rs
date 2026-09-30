//! #461 corpus: RAII across multiple return paths, sim-valued. One
//! run loops over every exit path; the destructor effect log pins what
//! each cleanup ran. Hand asserts in the `Pic18` sim (primary oracle).
//! The per-path call survival (no strand, no duplicate) is pinned
//! separately in `cpp_raii_preserve`.

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
fn cpp_raii_cleanup_values_on_every_path() {
    let fixture = "tests/fixtures/cpp_raii_sim.cpp";
    let hex_name = "tests/fixtures/cpp_raii_sim_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_raii_sim_p18f4550.map";
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture, "-o", hex_name, "--map", map_name, "--device", "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed on {fixture}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let map = std::fs::read_to_string(map_name).expect("read map");
    let _ = std::fs::remove_file(map_name);
    let addr = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string(hex_name).unwrap();
    let _ = std::fs::remove_file(hex_name);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "Pic18 sim should halt");
    let base = addr("out_vals");
    assert_eq!(&p.ram()[base..base + 3], &[10, 20, 30], "return values");
    let log = addr("g_log");
    assert_eq!(&p.ram()[log..log + 4], &[1, 2, 1, 1], "dtor effect order");
    assert_eq!(p.ram()[addr("out_n")], 4, "four cleanups ran");
}
