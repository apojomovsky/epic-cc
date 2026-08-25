// epic-cc#73 acceptance: calls through a function pointer, with the target
// selected at runtime, run correctly on both p16f877a (PIC14) and p18f4550
// (PIC18). `sel` drives a `select ptr @f0, ptr @f1, ptr @f2`; the call result
// lands in the volatile `out` global and the machine halts.
//
//   sel == 0 -> out = f0() = 10
//   sel == 1 -> out = f1() = 20
//   sel == 2 -> out = f2() = 30

use std::collections::HashMap;
use std::process::Command;

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
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/indirect_call.c",
            "-o",
            &hex_path,
            "--device",
            device_name,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} sel={sel}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();

    // Resolve the `sel`/`out` global addresses from the same alloc layout the
    // driver used.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/indirect_call.c"),
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
    let sel_addr = *layout.globals.get("sel").unwrap() as usize;
    let out_addr = *layout.globals.get("out").unwrap() as usize;

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
