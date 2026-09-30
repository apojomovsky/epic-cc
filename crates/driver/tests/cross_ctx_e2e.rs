// epic-cc#137 acceptance: a callback stored by main into a global the ISR
// reads (the cross-context shape) runs on both p16f877a (PIC14) and
// p18f4550 (PIC18). legalize duplicates the shared callback as `_isr` and
// rewrites main's store to the copy; the e2e fires the interrupt while
// main's own `on_event` call is in flight and checks the ISR's callback
// (the `_isr` copy) ran without corrupting main's live frame.
//
// Hand computation (in = 0x20):
//   main: g_cb = on_event_isr; r = on_event(0x10) -> marker = 0x33, r = 0x11
//   <- ISR fires here (main's on_event frame live)
//   ISR:  out = g_cb(in) -> on_event_isr(0x20) -> out = 0x21
//   main: out = r -> 0x11; marker = 0x22
//   out == 0x11, marker == 0x22, halted.
// If the ISR dispatched the main-context ORIGINAL, it would re-enter main's
// live frame, clobber the param slot, and main's call would return 0x21:
// out == 0x21 fails the assertion.

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
    let hex_path = format!("tests/fixtures/cross_ctx_{device_name}.hex");
    let map_path = format!("tests/fixtures/cross_ctx_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cross_ctx.c",
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
    let in_addr = map_addr(&map, "in");
    let out_addr = map_addr(&map, "out");
    let marker_addr = map_addr(&map, "marker");

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            p.ram_mut()[in_addr] = 0x20;
            // Run main to the point inside its `on_event(0x10)` call (the
            // marker store), where main's frame is live, then fire.
            let mut steps = 0usize;
            while p.ram()[marker_addr] != 0x33 {
                p.step();
                steps += 1;
                assert!(
                    steps < 200,
                    "never reached the marker=0x33 store (pc={})",
                    p.pc()
                );
            }
            p.fire_interrupt();
            assert_eq!(p.pc(), 4, "the ISR starts at the vector (word 4)");
            // The ISR runs (out = on_event_isr(0x20) = 0x21), RETFIE returns
            // to main's on_event (frame intact: r = 0x11), main writes
            // out = 0x11, marker = 0x22, then __start SLEEP halts.
            p.run(500_000);
            assert_eq!(
                p.ram()[out_addr],
                0x11,
                "PIC14: out == 0x11 (main's on_event returned 0x11; the ISR must have used the _isr copy)"
            );
            assert_eq!(
                p.ram()[marker_addr],
                0x22,
                "PIC14: marker == 0x22 (main completed)"
            );
            assert!(p.halted(), "PIC14 must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            p.ram_mut()[in_addr] = 0x20;
            let mut steps = 0usize;
            while p.ram()[marker_addr] != 0x33 {
                p.step();
                steps += 1;
                assert!(
                    steps < 1000,
                    "never reached the marker=0x33 store (pc={})",
                    p.pc()
                );
            }
            p.fire_interrupt();
            assert_eq!(p.pc(), 0x0008, "the ISR starts at the high vector");
            p.run(500_000);
            assert_eq!(
                p.ram()[out_addr],
                0x11,
                "PIC18: out == 0x11 (main's on_event returned 0x11; the ISR must have used the _isr copy)"
            );
            assert_eq!(
                p.ram()[marker_addr],
                0x22,
                "PIC18: marker == 0x22 (main completed)"
            );
            assert!(p.halted(), "PIC18 must halt");
        }
        device::Core::Pic14e => panic!("cross_ctx e2e: pic14e not implemented"),
        device::Core::PicBaseline => panic!("cross_ctx e2e: pic-baseline not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn cross_context_callback_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
