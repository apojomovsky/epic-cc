//! Generated headers acceptance (epic-cc#688): an XC8 tutorial blink per
//! beta core, through the whole pipeline and run in the simulator. `mid`
//! and `done` pin the `<REG>bits` reads; the two `__delay_ms(1)` calls at
//! 4 MHz contribute exactly 2000 instruction cycles, so the window below
//! proves the delays ran instead of compiling to nothing.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn build(fixture: &str, device_name: &str, stem: &str) -> (String, String) {
    let hex_path = format!("tests/fixtures/{stem}_{device_name}.hex");
    let map_path = format!("tests/fixtures/{stem}_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture,
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
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    (hex, map)
}

fn check_blink(
    hex: &str,
    map: &str,
    device: &device::Device,
    device_name: &str,
    delay_cycles: u64,
) {
    let mid_addr = map_addr(map, "mid");
    let done_addr = map_addr(map, "done");
    let window = delay_cycles..delay_cycles + 1000;
    match device.core {
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(hex));
            p.run(200_000);
            assert!(p.halted(), "blink must halt on {device_name}");
            assert_eq!(p.ram()[mid_addr], 1, "mid mismatch on {device_name}");
            assert_eq!(p.ram()[done_addr], 0, "done mismatch on {device_name}");
            assert!(
                window.contains(&p.cycles()),
                "cycles {} outside the delay window on {device_name}",
                p.cycles()
            );
        }
        device::Core::Pic14e => {
            let mut p = pic14_sim::Pic14e::with_device(device, pic14_sim::parse_hex_pic14e(hex));
            p.run(200_000);
            assert!(p.halted(), "blink must halt on {device_name}");
            assert_eq!(p.ram()[mid_addr], 1, "mid mismatch on {device_name}");
            assert_eq!(p.ram()[done_addr], 0, "done mismatch on {device_name}");
            assert!(
                window.contains(&p.cycles()),
                "cycles {} outside the delay window on {device_name}",
                p.cycles()
            );
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(hex));
            p.run(200_000);
            assert!(p.halted(), "blink must halt on {device_name}");
            assert_eq!(p.ram()[mid_addr], 1, "mid mismatch on {device_name}");
            assert_eq!(p.ram()[done_addr], 0, "done mismatch on {device_name}");
            assert!(
                window.contains(&p.cycles()),
                "cycles {} outside the delay window on {device_name}",
                p.cycles()
            );
        }
        other => panic!("blink: unsupported core {other:?}"),
    }
}

#[test]
fn xc_blink_runs_on_pic14() {
    let (hex, map) = build(
        "tests/fixtures/xc_blink_pic14.c",
        "p16f877a",
        "xc_blink_pic14",
    );
    check_blink(&hex, &map, &device::PIC16F877A, "p16f877a", 2000);
}

#[test]
fn xc_blink_runs_on_pic14e() {
    let (hex, map) = build(
        "tests/fixtures/xc_blink_pic14e.c",
        "p16f1937",
        "xc_blink_pic14e",
    );
    check_blink(
        &hex,
        &map,
        device::by_name("p16f1937").unwrap(),
        "p16f1937",
        2000,
    );
}

#[test]
fn xc_blink_runs_on_pic18() {
    let (hex, map) = build(
        "tests/fixtures/xc_blink_pic18.c",
        "p18f4550",
        "xc_blink_pic18",
    );
    check_blink(&hex, &map, &device::PIC18F4550, "p18f4550", 4000);
}

#[test]
fn every_interrupt_spelling_reaches_the_attribute() {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let opts = driver::clang::Options {
        defines: driver::predef::xc8_predefines(device::Core::Pic18, device::PIC18F4550.name),
        ..Default::default()
    };
    let ll = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/xc_interrupt.c"),
        &opts,
    );
    // Attribute groups are deduplicated, so resolve each function through
    // its own `#N` reference instead of counting group definitions.
    for (name, want) in [
        ("isr_empty", "0"),
        ("isr_hi", "1"),
        ("isr_lo", "2"),
        ("isr_num", "2"),
    ] {
        let def = ll
            .lines()
            .find(|l| l.contains(&format!("@{name}")) && l.contains('#'))
            .unwrap_or_else(|| panic!(".ll is missing {name}"));
        let group = def.rsplit('#').next().unwrap();
        let group = group.split_whitespace().next().unwrap();
        let attr = ll
            .lines()
            .find(|l| l.starts_with(&format!("attributes #{group} ")))
            .unwrap_or_else(|| panic!(".ll is missing attributes #{group}"));
        assert!(
            attr.contains(&format!("\"interrupt\"=\"{want}\"")),
            "{name} carries the wrong priority in: {attr}"
        );
    }
}
