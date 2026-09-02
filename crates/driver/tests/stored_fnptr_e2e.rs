//! epic-cc#155 acceptance: an indirect call through a function pointer stored
//! in a struct, with a runtime pointer arg (the epic-taskmgr `t->fn(t->arg)`
//! shape), runs correctly on both p16f877a (PIC14) and p18f4550 (PIC18).
//! The arg is a `load ptr` result (the TCB's arg field), not a compile-time
//! address: isel copies the loaded 2 bytes into the callee's param slot and
//! the callee's FSR-based deref resolves the address at runtime. Before the
//! fix both backends panicked ("no gep for pointer").
//!
//!   g_sel == 1 -> run_once(&g_tasks[0]) -> task_blink(g_tasks[0].arg)
//!              -> g_seen = *g_payload = 0xAB
//!   g_sel == 0 -> nothing; g_seen stays 0

use std::collections::HashMap;
use std::process::Command;

fn run_one(device_name: &str, device: &device::Device, sel: u8) {
    let hex_path = format!("tests/fixtures/stored_fnptr_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/stored_fnptr.c",
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

    // Resolve the `g_sel`/`g_seen` global addresses from the same alloc
    // layout the driver used.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/stored_fnptr.c"),
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
    let sel_addr = *layout.globals.get("g_sel").unwrap() as usize;
    let seen_addr = *layout.globals.get("g_seen").unwrap() as usize;

    match device.core {
        device::Core::Pic14 => {
            let prog = pic14_sim::parse_hex(&hex);
            let mut p = pic14_sim::Pic14::new(prog);
            p.ram_mut()[sel_addr] = sel;
            p.run(200_000);
            let expected = if sel != 0 { 0xAB } else { 0x00 };
            assert_eq!(
                p.ram()[seen_addr],
                expected,
                "PIC14 sel={sel} expected {expected:#04x} got {:#04x}",
                p.ram()[seen_addr]
            );
            assert!(p.halted(), "PIC14 sel={sel} must halt");
        }
        device::Core::Pic18 => {
            let prog = pic14_sim::parse_hex_pic18(&hex);
            let mut p = pic14_sim::Pic18::new(prog);
            p.ram_mut()[sel_addr] = sel;
            p.run(200_000);
            let expected = if sel != 0 { 0xAB } else { 0x00 };
            assert_eq!(
                p.ram()[seen_addr],
                expected,
                "PIC18 sel={sel} expected {expected:#04x} got {:#04x}",
                p.ram()[seen_addr]
            );
            assert!(p.halted(), "PIC18 sel={sel} must halt");
        }
        device::Core::Pic14e => panic!("stored_fnptr e2e: pic14e not implemented"),
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn stored_fnptr_runs_on_both_devices() {
    for sel in [0u8, 1] {
        run_one("p16f877a", &device::PIC16F877A, sel);
        run_one("p18f4550", &device::PIC18F4550, sel);
    }
}
