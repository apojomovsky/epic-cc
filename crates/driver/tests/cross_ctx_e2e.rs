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

use std::collections::HashMap;
use std::process::Command;

fn run_one(device_name: &str, device: &device::Device) {
    let hex_path = format!("tests/fixtures/cross_ctx_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cross_ctx.c",
            "-o",
            &hex_path,
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

    // Resolve the global addresses from the same alloc layout the driver
    // used.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/cross_ctx.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let in_addr = *layout.globals.get("in").unwrap() as usize;
    let out_addr = *layout.globals.get("out").unwrap() as usize;
    let marker_addr = *layout.globals.get("marker").unwrap() as usize;

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
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn cross_context_callback_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
