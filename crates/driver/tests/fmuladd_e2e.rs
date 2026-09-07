//! Regression for the `-ffp-contract=off` clang flag (epic-cc#267 PIC18
//! printf gap). At -O1 clang fuses `a*b+c` into a single `llvm.fmuladd`
//! intrinsic, which legalize does not handle (panic: unknown intrinsic).
//! Turning off FP contraction keeps `a*b+c` as separate fmul/fadd, which
//! the soft-float runtime lowers. Without the flag this program does not
//! compile; with it, `r` is computed in real arithmetic, not the fused
//! FMA, matching the runtime's exact fmul-then-fadd semantics.
//!
//! in = 3.0f (0x40400000); b = 1.5f*2.0f = 3.0f (folded constant);
//! r = b*in + 0.5f = 9.5f = 0x41180000; out = (unsigned char)((int)r & 0xFF).

use std::process::Command;

fn run_on(device_name: &str) {
    let hex_path =
        std::env::temp_dir().join(format!("fmuladd_{device_name}_{}.hex", std::process::id()));
    let map_path =
        std::env::temp_dir().join(format!("fmuladd_{device_name}_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", device_name, "--map"])
        .arg(&map_path)
        .args(["-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/fmuladd.c")
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
            if it.next() == Some("global") && it.next() == Some("out8") {
                it.next()
                    .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            } else {
                None
            }
        })
        .expect("out8 in map") as usize;

    let seed = |x: f32| x.to_bits().to_le_bytes();
    match device_name {
        "16F877A" => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&produced));
            for (i, b) in seed(3.0f32).iter().enumerate() {
                sim.ram_mut()[in_addr + i] = *b;
            }
            sim.run(300_000);
            assert_eq!(sim.ram()[out_addr], 9, "out mismatch on {device_name}");
        }
        "18F4550" => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&produced));
            for (i, b) in seed(3.0f32).iter().enumerate() {
                sim.ram_mut()[in_addr + i] = *b;
            }
            sim.run(300_000);
            assert_eq!(sim.ram()[out_addr], 9, "out mismatch on {device_name}");
        }
        other => panic!("fmuladd: unsupported target {other}"),
    }
}

#[test]
fn fmuladd_runs_on_p16() {
    run_on("16F877A");
}

#[test]
fn fmuladd_runs_on_p18() {
    run_on("18F4550");
}
