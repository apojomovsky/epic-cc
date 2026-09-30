// epic-cc#137: the param-forwarded registration shape (the HAL's
// `EPIC_GPIO_RegisterChangeCallback(on_rb_change)` pattern) runs on both
// p16f877a (PIC14) and p18f4550 (PIC18). main passes the callback as a call
// argument; legalize rewrites the argument to the `_isr` copy and isel
// materializes the function address as LOW/HIGH literals in the untyped ptr
// arg path. The e2e fires the interrupt mid-run and checks the ISR's
// callback ran and wrote the expected value.
//
// Hand computation:
//   main: register(on_event_isr); out = 0x11
//   <- ISR fires here
//   ISR:  if (g_cb) g_cb() -> on_event_isr: out = 0x55
//   main: __start SLEEP halts the machine
//   out == 0x55, halted.

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
    let hex_path = format!("tests/fixtures/cross_ctx_param_{device_name}.hex");
    let map_path = format!("tests/fixtures/cross_ctx_param_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cross_ctx_param.c",
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
    let out_addr = map_addr(&map, "out");

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            // Run main to the point right after the `out = 0x11` store (the
            // registration precedes it), then fire the interrupt.
            let mut steps = 0usize;
            while p.ram()[out_addr] != 0x11 {
                p.step();
                steps += 1;
                assert!(
                    steps < 200,
                    "never reached the out=0x11 store (pc={})",
                    p.pc()
                );
            }
            p.fire_interrupt();
            assert_eq!(p.pc(), 4, "the ISR starts at the vector (word 4)");
            p.run(500_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC14: the ISR's callback must have run (out = 0x55)"
            );
            assert!(p.halted(), "PIC14 must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            let mut steps = 0usize;
            while p.ram()[out_addr] != 0x11 {
                p.step();
                steps += 1;
                assert!(
                    steps < 1000,
                    "never reached the out=0x11 store (pc={})",
                    p.pc()
                );
            }
            p.fire_interrupt();
            assert_eq!(p.pc(), 0x0008, "the ISR starts at the high vector");
            p.run(500_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC18: the ISR's callback must have run (out = 0x55)"
            );
            assert!(p.halted(), "PIC18 must halt");
        }
        device::Core::Pic14e => panic!("cross_ctx_param e2e: pic14e not implemented"),
        device::Core::PicBaseline => panic!("cross_ctx_param e2e: pic-baseline not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn param_forwarded_callback_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
