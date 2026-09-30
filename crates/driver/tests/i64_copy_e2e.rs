//! epic-cc#125 acceptance: a whole-struct copy of a multi-byte handle (the
//! HAL's `g_t2_storage = *h` pattern) runs correctly on both p16f877a
//! (PIC14, `load i64`/`store i64`) and p18f4550 (PIC18, `llvm.memcpy` with
//! an indirect source). The copied callback pointer must dispatch through
//! the storage copy: `g_out == 0x55`, halted.

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

fn run_one(device_name: &str, device: &device::Device) {
    let hex_path = format!("tests/fixtures/i64_copy_{device_name}.hex");
    let map_path = format!("tests/fixtures/i64_copy_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/i64_copy.c",
            "-o",
            &hex_path,
            "--map",
            &map_path,
            "--device",
            device_name,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "g_out");

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC14: the copied callback must dispatch through g_storage"
            );
            assert!(p.halted(), "PIC14 must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC18: the copied callback must dispatch through g_storage"
            );
            assert!(p.halted(), "PIC18 must halt");
        }
        device::Core::Pic14e => panic!("i64_copy e2e: pic14e not implemented"),
        device::Core::PicBaseline => panic!("i64_copy e2e: pic-baseline not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn i64_aggregate_copy_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
