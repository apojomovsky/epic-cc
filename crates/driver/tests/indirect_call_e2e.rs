// epic-cc#73 acceptance: calls through a function pointer, with the target
// selected at runtime, run correctly on both p16f877a (PIC14) and p18f4550
// (PIC18). `sel` drives a `select ptr @f0, ptr @f1, ptr @f2`; the call result
// lands in the volatile `out` global and the machine halts.
//
//   sel == 0 -> out = f0() = 10
//   sel == 1 -> out = f1() = 20
//   sel == 2 -> out = f2() = 30

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

fn expected(sel: u8) -> u8 {
    match sel {
        0 => 10,
        1 => 20,
        2 => 30,
        _ => panic!("unexpected sel {sel}"),
    }
}

fn run_one(device_name: &str, device: &device::Device, sel: u8) {
    let hex_path = format!("tests/fixtures/indirect_call_{device_name}.hex");
    let map_path = format!("tests/fixtures/indirect_call_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call.c",
            "-o",
            &hex_path,
            "--map",
            &map_path,
            "--device",
            device_name,
            // `sel`'s input rides in as a real initializer (`volatile
            // unsigned char sel = SEL`): __start clears zero-initialized
            // globals before main (epic-cc#561), which would erase a
            // sim-side seed.
            "-D",
        ])
        .arg(format!("SEL={sel}"))
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} sel={sel}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let sel_addr = map_addr(&map, "sel");
    let out_addr = map_addr(&map, "out");

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            p.ram_mut()[sel_addr] = sel;
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                expected(sel),
                "PIC14 sel={sel} expected {} got {}",
                expected(sel),
                p.ram()[out_addr]
            );
            assert!(p.halted(), "PIC14 sel={sel} must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            p.ram_mut()[sel_addr] = sel;
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                expected(sel),
                "PIC18 sel={sel} expected {} got {}",
                expected(sel),
                p.ram()[out_addr]
            );
            assert!(p.halted(), "PIC18 sel={sel} must halt");
        }
        device::Core::Pic14e => panic!("indirect_call e2e: pic14e not implemented"),
        device::Core::PicBaseline => panic!("indirect_call e2e: pic-baseline not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn indirect_call_runs_on_both_devices() {
    for sel in [0u8, 1, 2] {
        run_one("p16f877a", &device::PIC16F877A, sel);
        run_one("p18f4550", &device::PIC18F4550, sel);
    }
}
