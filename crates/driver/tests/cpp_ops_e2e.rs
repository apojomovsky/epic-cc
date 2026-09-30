//! #461 corpus: operator overloading as the mangling/ABI guard.
//! Hand asserts in the `Pic18` sim (primary oracle, docs/40 §5).

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
fn cpp_operator_overloads_evaluate() {
    let fixture = "tests/fixtures/cpp_ops.cpp";
    let hex_name = "tests/fixtures/cpp_ops_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_ops_p18f4550.map";
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
    assert_eq!(p.ram()[addr("out_sum")], 7, "3 + 4");
    assert_eq!(p.ram()[addr("out_eq")], 0, "a != b");
    assert_eq!(p.ram()[addr("out_ne")], 1, "a == a");
}
