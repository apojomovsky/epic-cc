//! #460 acceptance: single-inheritance virtual dispatch through a
//! flash-resident vtable (ADR-010): the vptr slot address folds at parse,
//! the slot load seeds `TBLPTR` from the vptr value (#832), and the
//! indirect call dispatches the selected override. The input class selects
//! the dynamic type at runtime (`-D SEL=`); all three overrides return from
//! the same vtable layout. PIC18 only.
//! The dispatch sequence replays on hardware through the `tblrd-flash-ptr`
//! superopt spec; per-fixture runs stay sim-only (docs/40 §5).

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn run_sel(sel: u8, expect: u8) {
    let fixture = "tests/fixtures/cpp_virtual.cpp";
    let hex_name = format!("tests/fixtures/cpp_virtual_{sel}_p18f4550.hex");
    let map_name = format!("tests/fixtures/cpp_virtual_{sel}_p18f4550.map");
    // The selector rides in as a real initializer (`__start` clears
    // zero-initialized globals before main, epic-cc#561).
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture, "-o", &hex_name, "--map", &map_name, "--device", "p18f4550", "-D",
        ])
        .arg(format!("SEL={sel}"))
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed on {fixture} sel={sel}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let map = std::fs::read_to_string(&map_name).expect("read map");
    let _ = std::fs::remove_file(&map_name);
    let addr = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string(&hex_name).unwrap();
    let _ = std::fs::remove_file(&hex_name);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "Pic18 sim should halt (sel={sel})");
    assert_eq!(
        p.ram()[addr("out_val")],
        expect,
        "sel={sel} must dispatch the right override"
    );
}

#[test]
fn cpp_virtual_dispatch_base() {
    run_sel(0, 0);
}

#[test]
fn cpp_virtual_dispatch_derived() {
    run_sel(1, 1);
}

#[test]
fn cpp_virtual_dispatch_third() {
    run_sel(2, 2);
}

#[test]
fn cpp_virtual_devirtualized_single_type() {
    let fixture = "tests/fixtures/cpp_devirt.cpp";
    let hex_name = "tests/fixtures/cpp_devirt_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_devirt_p18f4550.map";
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
    let hex = std::fs::read_to_string(hex_name).unwrap();
    let _ = std::fs::remove_file(hex_name);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "Pic18 sim should halt");
    assert_eq!(
        p.ram()[map_addr(&map, "out_val")],
        7,
        "derived override value"
    );
}
