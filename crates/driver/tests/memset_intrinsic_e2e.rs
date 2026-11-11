//! `llvm.memset` intrinsic acceptance (epic-cc#1448): clang turns
//! struct zero-initialization and `__builtin_memset` into the intrinsic,
//! which used to stop legalize with an internal compiler error. The
//! fixture includes no `<string.h>`, so the run also proves the driver
//! links a memset implementation when the intrinsic appears. Expected
//! values are hand-computed in the fixture.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    for line in map.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("global") && it.next() == Some(name) {
            let addr = it.next().expect("map addr");
            return usize::from_str_radix(addr.trim_start_matches("0x"), 16).unwrap();
        }
    }
    panic!("no global {name} in map:\n{map}");
}

fn run_on(device_name: &str, device: &device::Device) {
    run_fixture(
        "tests/fixtures/memset_intrinsic.c",
        "plain",
        device_name,
        device,
    );
}

fn run_fixture(fixture: &str, tag_name: &str, device_name: &str, device: &device::Device) {
    let tag = format!("memset_{tag_name}_{device_name}_{}", std::process::id());
    let hex_path = std::env::temp_dir().join(format!("{tag}.hex"));
    let map_path = std::env::temp_dir().join(format!("{tag}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "out");
    let expected: u8 = 0xAA;
    match device.core {
        device::Core::Pic14 => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            sim.run(200_000);
            assert!(sim.halted(), "halted {device_name}");
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name}");
        }
        device::Core::Pic18 => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            sim.run(200_000);
            assert!(sim.halted(), "halted {device_name}");
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name}");
        }
        device::Core::Pic14e => {
            let mut sim = pic14_sim::Pic14e::with_device(device, pic14_sim::parse_hex(&hex));
            sim.run(200_000);
            assert!(sim.halted(), "halted {device_name}");
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name}");
        }
        other => panic!("memset_intrinsic: unsupported core {other:?}"),
    }
}

#[test]
fn memset_intrinsic_runs_on_p18() {
    run_on("p18f4550", &device::PIC18F4550);
}

#[test]
fn memset_intrinsic_runs_on_p14e() {
    run_on("p16f1938", &device::PIC16F1938);
}

#[test]
fn memset_intrinsic_runs_on_p16() {
    run_on("p16f877a", &device::PIC16F877A);
}

#[test]
fn memset_intrinsic_with_string_h_runs_on_p18() {
    run_fixture(
        "tests/fixtures/memset_intrinsic_string_h.c",
        "stringh",
        "p18f4550",
        &device::PIC18F4550,
    );
}

#[test]
fn memset_intrinsic_with_string_h_runs_on_p14e() {
    run_fixture(
        "tests/fixtures/memset_intrinsic_string_h.c",
        "stringh",
        "p16f1938",
        &device::PIC16F1938,
    );
}
