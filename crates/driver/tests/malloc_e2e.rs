//! malloc/free acceptance (epic-cc#343): the freestanding first-fit heap
//! over a caller-registered arena (`_initHeap`, SDCC pic16's model),
//! compiled through the whole pipeline and run on all three cores' simulators.
//!
//! `fixtures/malloc.c` allocates two blocks, writes through both, frees
//! one, allocates a third, and folds the observable bytes: with
//! b = {0x33, 0x44} and c = {0x55, 0x66}, out = b[0]^b[3]^c[0]^c[2]^0x7F
//! = 0x3B. A zero `out` instead means an allocation failed (the probe's
//! own OOM sentinel), and any other value is a heap corruption.

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
    let fixture = "tests/fixtures/malloc.c";
    let tag = format!("malloc_{device_name}_{}", std::process::id());
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
    let expected: u8 = 0x3B;
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
        other => panic!("malloc: unsupported core {other:?}"),
    }
}

#[test]
fn malloc_runs_on_p16() {
    run_on("p16f877a", &device::PIC16F877A);
}

#[test]
fn malloc_runs_on_p18() {
    run_on("p18f4550", &device::PIC18F4550);
}

#[test]
fn malloc_runs_on_p14e() {
    run_on("p16f1938", &device::PIC16F1938);
}
