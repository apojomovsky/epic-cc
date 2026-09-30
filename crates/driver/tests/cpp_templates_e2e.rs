//! #461 corpus: class and function templates instantiated for two
//! types. Each used specialization reaches IR on its own; the test pins
//! both values. Hand asserts in the `Pic18` sim (primary oracle).

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
fn cpp_templates_instantiate_for_two_types() {
    let fixture = "tests/fixtures/cpp_templates.cpp";
    let hex_name = "tests/fixtures/cpp_templates_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_templates_p18f4550.map";
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
    assert_eq!(p.ram()[addr("out_b8")], 3, "Box u8");
    assert_eq!(p.ram()[addr("out_b16lo")], 0x34, "Box u16 low");
    assert_eq!(p.ram()[addr("out_b16hi")], 0x12, "Box u16 high");
    assert_eq!(p.ram()[addr("out_a8")], 11, "add2 u8");
    assert_eq!(p.ram()[addr("out_a16lo")], 0x20, "add2 u16 low");
    assert_eq!(p.ram()[addr("out_a16hi")], 0x10, "add2 u16 high");
}
