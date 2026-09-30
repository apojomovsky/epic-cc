//! #799 acceptance: C++ class records (`%class.Acc`, quoted
//! `%"class.cfg::Scale"`) parse and lower end to end. The fixture uses
//! methods, a namespace, and globals only: constructors, destructors,
//! and virtuals are later tickets' input (#458, #460) and stay out.
//! PIC18 only, per the EC++ epic scope.

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
fn cpp_class_methods_run_on_pic18() {
    let fixture = "tests/fixtures/cpp_class.cpp";
    let hex_name = "tests/fixtures/cpp_class_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_class_p18f4550.map";
    // The input rides in as a real initializer (`__start` clears
    // zero-initialized globals before main, epic-cc#561).
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture, "-o", hex_name, "--map", map_name, "--device", "p18f4550", "-D", "IN_VAL=7",
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
    // out = scale.apply(acc.get()) = (in + 0) + 2 = 9.
    assert_eq!(p.ram()[addr("out_val")], 0x09, "method calls agree");
}
