//! `%f` printf acceptance (epic-cc#267 PIC18 printf gap). Formats an f32
//! (== double on msp430) at 2 fixed decimals via the soft-float runtime
//! (fcmp/fsub/fmul/fptoui). The expected string is the host-computed
//! value, so the sim result is the differential check.
//!
//! in = 3.5f (0x40600000): whole = 3, frac = 0.5, cent = round(50.0) = 50,
//! so the output is "d=3.50\r\n".

use std::process::Command;

const EXPECTED: &str = "d=3.50\r\n";

fn run_on(device_name: &str) {
    let hex_path =
        std::env::temp_dir().join(format!("printf_f_{device_name}_{}.hex", std::process::id()));
    let map_path =
        std::env::temp_dir().join(format!("printf_f_{device_name}_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", device_name, "--map"])
        .arg(&map_path)
        .args(["-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/printf_f.c")
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
    let buf_addr = map
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next() == Some("global") && it.next() == Some("g_buf") {
                it.next()
                    .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            } else {
                None
            }
        })
        .expect("g_buf in map") as usize;

    match device_name {
        "16F877A" => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&produced));
            sim.run(300_000);
            let buf: Vec<u8> = sim.ram()[buf_addr..buf_addr + EXPECTED.len() + 1].to_vec();
            let s = String::from_utf8_lossy(&buf);
            assert!(
                s.starts_with(EXPECTED),
                "output mismatch on {device_name}: {s:?}"
            );
        }
        "18F4550" => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&produced));
            sim.run(300_000);
            let buf: Vec<u8> = sim.ram()[buf_addr..buf_addr + EXPECTED.len() + 1].to_vec();
            let s = String::from_utf8_lossy(&buf);
            assert!(
                s.starts_with(EXPECTED),
                "output mismatch on {device_name}: {s:?}"
            );
        }
        other => panic!("printf_f: unsupported target {other}"),
    }
}

#[test]
fn printf_f_runs_on_p16() {
    run_on("16F877A");
}

#[test]
fn printf_f_runs_on_p18() {
    run_on("18F4550");
}
