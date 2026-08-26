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

use std::collections::HashMap;
use std::process::Command;

fn run_one(device_name: &str, device: &device::Device) {
    let hex_path = format!("tests/fixtures/cross_ctx_param_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cross_ctx_param.c",
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

    // Resolve the `out` global address from the same alloc layout the driver
    // used.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/cross_ctx_param.c"),
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
    let out_addr = *layout.globals.get("out").unwrap() as usize;

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
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn param_forwarded_callback_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
