//! Unions acceptance (PIC14 parity, #268): a union of u8[2]/u16, written
//! through one member and read through the other, both directions, on both
//! cores. The differential probe (crates/sdcc-parity) already passes; this
//! pins the behavior as a committed e2e test.
//!
//! in = 0x1234: v.w = in, out = v.b[0] + v.b[1] = 0x34 + 0x12 = 0x46
//! (little endian). Then v.b[0] = 0x34, v.b[1] = 0x12, out2 = v.w & 0xFF =
//! 0x34.

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

fn run_on(device: &device::Device, device_name: &str) {
    let hex_path =
        std::env::temp_dir().join(format!("unions_{device_name}_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/unions.c")
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
    let out2_addr = map_addr(&map, "out2");

    match device.core {
        // `in` arrives initialized to 0x1234 via the fixture's init store
        // (epic-cc#561): __start clears zero-initialized globals, so a
        // sim-side seed would not survive.
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            p.run(200_000);
            assert_eq!(p.ram()[out_addr], 0x46, "out mismatch on {device_name}");
            assert_eq!(p.ram()[out2_addr], 0x34, "out2 mismatch on {device_name}");
            assert!(p.halted());
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            p.run(200_000);
            assert_eq!(p.ram()[out_addr], 0x46, "out mismatch on {device_name}");
            assert_eq!(p.ram()[out2_addr], 0x34, "out2 mismatch on {device_name}");
            assert!(p.halted());
        }
        other => panic!("unions: unsupported core {other:?}"),
    }
}

#[test]
fn unions_runs_on_p16() {
    run_on(&device::PIC16F877A, "p16f877a");
}

#[test]
fn unions_runs_on_p18() {
    run_on(&device::PIC18F4550, "p18f4550");
}
