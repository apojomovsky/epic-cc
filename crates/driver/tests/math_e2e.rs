//! `math.h` acceptance (epic-cc#344). The four f32 routines ship as our
//! own plain-C implementations over the soft-float runtime; this test
//! pins each routine on an exact hand-computed value on both cores.
//! All values are exact in f32, so the host computation is bit-identical
//! to a correct target run.

use std::process::Command;

fn f32_at(sim: &[u8], addr: usize) -> [u8; 4] {
    [sim[addr], sim[addr + 1], sim[addr + 2], sim[addr + 3]]
}

fn addr_of(map: &str, name: &str) -> usize {
    map.lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next() == Some("global") && it.next() == Some(name) {
                it.next()
                    .and_then(|a| u16::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("{name} in map")) as usize
}

fn run_on(device_name: &str) {
    let hex_path =
        std::env::temp_dir().join(format!("math_{device_name}_{}.hex", std::process::id()));
    let map_path =
        std::env::temp_dir().join(format!("math_{device_name}_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", device_name, "--map"])
        .arg(&map_path)
        .args(["-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/math.c")
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

    let check = |ram: &[u8]| {
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_sqrt")),
            1.5f32.to_bits().to_le_bytes(),
            "sqrtf(2.25) == 1.5 on {device_name}"
        );
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_sqrt_small")),
            0.5f32.to_bits().to_le_bytes(),
            "sqrtf(0.25) == 0.5 on {device_name}"
        );
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_fabs")),
            2.25f32.to_bits().to_le_bytes(),
            "fabsf(-2.25) == 2.25 on {device_name}"
        );
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_fmax")),
            2.25f32.to_bits().to_le_bytes(),
            "fmaxf(1.5, 2.25) == 2.25 on {device_name}"
        );
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_floor")),
            2.0f32.to_bits().to_le_bytes(),
            "floorf(2.99) == 2.0 on {device_name}"
        );
        assert_eq!(
            f32_at(ram, addr_of(&map, "out_floor_neg")),
            (-3.0f32).to_bits().to_le_bytes(),
            "floorf(-2.25) == -3.0 on {device_name}"
        );
    };
    match device_name {
        "16F877A" => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&produced));
            sim.run(300_000);
            check(sim.ram());
        }
        "18F4550" => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&produced));
            sim.run(300_000);
            check(sim.ram());
        }
        other => panic!("math: unsupported target {other}"),
    }
}

#[test]
fn math_runs_on_p16() {
    run_on("16F877A");
}

#[test]
fn math_runs_on_p18() {
    run_on("18F4550");
}
