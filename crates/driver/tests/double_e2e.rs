//! double acceptance: `double` is 32-bit on msp430 (same as float), so
//! clang emits `double` as f32-width IR. This test pins the double type
//! mapping end-to-end: a double global, double arithmetic, and a
//! double->unsigned int conversion, on both cores.
//!
//! Expected (in = 2): a = 1.5, b = (double)in = 2.0, c = a * b = 3.0,
//! out = (unsigned int)c & 0xFF = 3.

use std::process::Command;

fn run_on(device_name: &str) {
    let hex_path =
        std::env::temp_dir().join(format!("double_{device_name}_{}.hex", std::process::id()));
    let map_path =
        std::env::temp_dir().join(format!("double_{device_name}_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", device_name, "--map"])
        .arg(&map_path)
        .args(["-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/double.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let _ = std::fs::remove_file(&hex_path);
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let in_addr = map
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next() == Some("global") && it.next() == Some("in") {
                it.next()
                    .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            } else {
                None
            }
        })
        .expect("in in map") as usize;
    let out_addr = map
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next() == Some("global") && it.next() == Some("out") {
                it.next()
                    .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            } else {
                None
            }
        })
        .expect("out in map") as usize;

    match device_name {
        "16F877A" => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&produced));
            // Seed `in` = 2.0f (0x40000000) as 4 little-endian bytes.
            let in_bytes = 0x4000_0000u32.to_le_bytes();
            for (i, b) in in_bytes.iter().enumerate() {
                sim.ram_mut()[in_addr + i] = *b;
            }
            sim.run(300_000);
            assert_eq!(sim.ram()[out_addr], 3, "out mismatch on {device_name}");
        }
        "18F4550" => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&produced));
            let in_bytes = 0x4000_0000u32.to_le_bytes();
            for (i, b) in in_bytes.iter().enumerate() {
                sim.ram_mut()[in_addr + i] = *b;
            }
            sim.run(300_000);
            assert_eq!(sim.ram()[out_addr], 3, "out mismatch on {device_name}");
        }
        other => panic!("double: unsupported target {other}"),
    }
}

#[test]
fn double_runs_on_p16() {
    run_on("16F877A");
}

#[test]
fn double_runs_on_p18() {
    run_on("18F4550");
}
