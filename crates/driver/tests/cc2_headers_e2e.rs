//! CC-2 acceptance: the freestanding headers (`stdint.h`, `stdbool.h`,
//! `stddef.h`, `stdlib.h`, `string.h`) and the `string.h` implementation the driver links
//! in when a source includes it, compiled through the whole pipeline and run
//! on both cores' simulators.
//!
//! `fixtures/cc2_string.c` exercises every implemented `<string.h>` entry
//! point and sums one point per passing check into `out`; the expected total
//! is hand-computed in the fixture. `memmove` gets overlapping ranges, the
//! check that pins the back-to-front copy and the pointer comparison
//! selecting it.

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

fn run_cc2(device_name: &str, device: &device::Device) {
    let fixture = "tests/fixtures/cc2_string.c";
    let hex_path = format!("tests/fixtures/cc2_string_{device_name}.hex");
    let map_path = format!("tests/fixtures/cc2_string_{device_name}.map");

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
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "out");

    // in = 7, so every check passing sums to 26 (see the fixture).
    let expected: u8 = 26;
    match device.core {
        device::Core::Pic14 => {
            // `in` arrives initialized to 7 (the fixture), so the program
            // itself carries the input: __start clears zero-initialized
            // globals before main (epic-cc#561), which would erase a
            // sim-side seed.
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            sim.run(200_000);
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name} in=7");
            assert!(sim.halted(), "halted {device_name}");
        }
        device::Core::Pic18 => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            sim.run(200_000);
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name} in=7");
            assert!(sim.halted(), "halted {device_name}");
        }
        device::Core::Pic14e => panic!(
            "cc2 test: pic14e core not yet implemented for {}",
            device.name
        ),
        device::Core::PicBaseline => panic!(
            "cc2 test: pic-baseline core not yet implemented for {}",
            device.name
        ),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn cc2_string_runs_on_p16() {
    run_cc2("p16f877a", &device::PIC16F877A);
}

#[test]
fn cc2_string_runs_on_p18() {
    run_cc2("p18f4550", &device::PIC18F4550);
}

#[test]
fn cc2_headers_compile_without_string() {
    // The type headers must stand alone: a source that never includes
    // <string.h> gets no extra translation unit linked in.
    let src = r#"
        #include <stdint.h>
        #include <stdbool.h>
        #include <stddef.h>
        #include <stdlib.h>
        volatile uint8_t in;
        volatile uint8_t out;
        void main(void) { bool b = true; size_t n = 1; uint16_t x = in; out = b ? (uint8_t)(x + n) : 0; }
    "#;
    let tmp = std::env::temp_dir().join("cc2_no_string.c");
    std::fs::write(&tmp, src).unwrap();
    let hex_path = std::env::temp_dir().join("cc2_no_string.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            tmp.to_str().unwrap(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver without string.h: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&hex_path);
}
