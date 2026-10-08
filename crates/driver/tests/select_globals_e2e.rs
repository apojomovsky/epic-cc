//! Pointer selects across distinct global addresses must compile and read
//! the selected arm's byte on both cores: two globals (epic-cc#147), one
//! const arm against RAM (epic-cc#147), and a nonzero-offset GEP arm
//! (epic-cc#781). Each fixture carries its own expectations; the harness
//! below compiles with `-D OK_FLAG=n` and checks `out` in the sim.
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

/// Compile `fixture` for `device` and run it in the sim with `ok_flag`
/// compiled in as a real initializer for each `(flag, expected_out)` pair,
/// asserting `out` and `halted`. The flag rides in via `-D OK_FLAG=n`
/// (epic-cc#561: __start clears zero-initialized globals, so a sim-side
/// seed would not survive).
fn run_fixture(
    device_name: &str,
    device: &'static device::Device,
    fixture: &str,
    cases: &[(u8, u8)],
) {
    for &(flag, expected) in cases {
        let stem = std::path::Path::new(fixture)
            .file_stem()
            .expect("fixture file name")
            .to_str()
            .expect("fixture name utf8");
        let hex_path = format!("tests/fixtures/{stem}_{device_name}_{flag}.hex");
        let map_path = format!("tests/fixtures/{stem}_{device_name}_{flag}.map");
        let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args([
                fixture,
                "-o",
                &hex_path,
                "--map",
                &map_path,
                "--device",
                device_name,
                "-D",
            ])
            .arg(format!("OK_FLAG={flag}"))
            .output()
            .expect("run driver");
        assert!(
            out.status.success(),
            "driver {device_name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let hex = std::fs::read_to_string(&hex_path).expect("read hex");
        let map = std::fs::read_to_string(&map_path).expect("read map");
        let _ = std::fs::remove_file(&map_path);
        let out_addr = map_addr(&map, "out");

        match device.core {
            device::Core::Pic14 => {
                let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
                sim.run(200_000);
                assert_eq!(
                    sim.ram()[out_addr],
                    expected,
                    "out {device_name} flag={flag}"
                );
                assert!(sim.halted(), "halted {device_name} flag={flag}");
            }
            device::Core::Pic18 => {
                let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
                sim.run(200_000);
                assert_eq!(
                    sim.ram()[out_addr],
                    expected,
                    "out {device_name} flag={flag}"
                );
                assert!(sim.halted(), "halted {device_name} flag={flag}");
            }
            device::Core::Pic14e => panic!("pic14e core not implemented"),
            device::Core::PicBaseline => panic!("pic-baseline core not implemented"),
        }
        let _ = std::fs::remove_file(&hex_path);
    }
}

fn run_select_globals(device_name: &str, device: &'static device::Device) {
    run_fixture(
        device_name,
        device,
        "tests/fixtures/select_globals.c",
        &[(1, b'P'), (0, b'F')],
    );
}

fn run_select_globals_one_const(device_name: &str, device: &'static device::Device) {
    run_fixture(
        device_name,
        device,
        "tests/fixtures/select_globals_one_const.c",
        &[(1, b'P'), (0, b'R')],
    );
}

fn run_select_gep_offset(device_name: &str, device: &'static device::Device) {
    // epic-cc#781: one arm is a nonzero-offset GEP over a const, the
    // other a RAM global. The arms share no base, so the select seeds
    // as an indirect slot and each backend materializes base plus k.
    run_fixture(
        device_name,
        device,
        "tests/fixtures/select_gep_offset.c",
        &[(1, b'b'), (0, b'R')],
    );
}

#[test]
fn select_globals_runs_on_p16() {
    run_select_globals("p16f877a", &device::PIC16F877A);
}

#[test]
fn select_globals_runs_on_p18() {
    run_select_globals("p18f4550", &device::PIC18F4550);
}

#[test]
fn select_globals_one_const_arm_runs_on_p16() {
    run_select_globals_one_const("p16f877a", &device::PIC16F877A);
}

#[test]
fn select_globals_one_const_arm_runs_on_p18() {
    run_select_globals_one_const("p18f4550", &device::PIC18F4550);
}

#[test]
fn select_gep_offset_runs_on_p16() {
    run_select_gep_offset("p16f877a", &device::PIC16F877A);
}

#[test]
fn select_gep_offset_runs_on_p18() {
    run_select_gep_offset("p18f4550", &device::PIC18F4550);
}
