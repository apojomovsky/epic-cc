//! #665 acceptance: byte-indexed static arrays lower through `LFSR` +
//! `PLUSW0` on PIC18 and behave exactly. The index rides in as an
//! initializer via IDX so `__start`'s zero-init cannot erase it. IDX
//! stays at or below 14 so `i + 1` and `15 - i` are always in bounds.
//! Addresses come from the driver's own `--map`, never a re-derived
//! layout.

use std::collections::HashMap;
use std::process::Command;

fn expected(idx: u8) -> (u8, u8, u8, u8) {
    let i = idx as u16;
    (
        (3 * i + 1) as u8,
        (3 * i + 4) as u8,
        (3 * i + 2) as u8,
        (46 - 3 * i) as u8,
    )
}

#[test]
fn plusw_indexed_arrays_read_and_write_correctly() {
    for idx in [0u8, 5, 14] {
        let hex_name = format!("tests/fixtures/plusw_indexed_18F4550_{idx}.hex");
        let map_name = format!("tests/fixtures/plusw_indexed_18F4550_{idx}.map");
        let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args([
                "tests/fixtures/plusw_indexed.c",
                "-o",
                &hex_name,
                "--device",
                "p18f4550",
            ])
            .arg("--map")
            .arg(&map_name)
            .arg("-D")
            .arg(format!("IDX={idx}"))
            .output()
            .expect("run driver");
        assert!(
            output.status.success(),
            "driver failed for IDX={idx}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut addrs = HashMap::new();
        for line in std::fs::read_to_string(&map_name).unwrap().lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some("global") {
                let name = parts.next().expect("map name").to_string();
                let addr = parts.next().expect("map addr");
                addrs.insert(
                    name,
                    u16::from_str_radix(addr.trim_start_matches("0x"), 16).unwrap(),
                );
            }
        }
        let addr = |n: &str| *addrs.get(n).expect(n) as usize;
        let hex = std::fs::read_to_string(&hex_name).unwrap();
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
        p.run(200_000);
        assert!(p.halted(), "must halt for IDX={idx}");
        let (e0, e1, e2, e3) = expected(idx);
        assert_eq!(p.ram()[addr("g_out0")], e0, "out0 arr[i] for IDX={idx}");
        assert_eq!(p.ram()[addr("g_out1")], e1, "out1 arr[i+1] for IDX={idx}");
        assert_eq!(
            p.ram()[addr("g_out2")],
            e2,
            "out2 store readback for IDX={idx}"
        );
        assert_eq!(p.ram()[addr("g_out3")], e3, "out3 arr[15-i] for IDX={idx}");
        let _ = std::fs::remove_file(&hex_name);
        let _ = std::fs::remove_file(&map_name);
    }
}
