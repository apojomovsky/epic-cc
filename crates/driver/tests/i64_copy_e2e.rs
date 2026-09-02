//! epic-cc#125 acceptance: a whole-struct copy of a multi-byte handle (the
//! HAL's `g_t2_storage = *h` pattern) runs correctly on both p16f877a
//! (PIC14, `load i64`/`store i64`) and p18f4550 (PIC18, `llvm.memcpy` with
//! an indirect source). The copied callback pointer must dispatch through
//! the storage copy: `g_out == 0x55`, halted.

use std::collections::HashMap;
use std::process::Command;

fn run_one(device_name: &str, device: &device::Device) {
    let hex_path = format!("tests/fixtures/i64_copy_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/i64_copy.c",
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

    // Resolve the `g_out` global address from the same alloc layout the
    // driver used.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/i64_copy.c"),
        &driver::clang::Options {
            // The driver packs structs on PIC18 (XC8 record layout); the
            // layout must match the driver's for the address lookup.
            packed_structs: device.core == device::Core::Pic18,
            ..Default::default()
        },
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let out_addr = *layout.globals.get("g_out").unwrap() as usize;

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC14: the copied callback must dispatch through g_storage"
            );
            assert!(p.halted(), "PIC14 must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            p.run(200_000);
            assert_eq!(
                p.ram()[out_addr],
                0x55,
                "PIC18: the copied callback must dispatch through g_storage"
            );
            assert!(p.halted(), "PIC18 must halt");
        }
        device::Core::Pic14e => panic!("i64_copy e2e: pic14e not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn i64_aggregate_copy_runs_on_both_devices() {
    run_one("p16f877a", &device::PIC16F877A);
    run_one("p18f4550", &device::PIC18F4550);
}
