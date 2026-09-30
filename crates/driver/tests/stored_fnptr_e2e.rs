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

fn run_one(device_name: &str, device: &device::Device, sel: u8) {
    let hex_path = format!("tests/fixtures/stored_fnptr_{device_name}.hex");
    let map_path = format!("tests/fixtures/stored_fnptr_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/stored_fnptr.c",
            "-o",
            &hex_path,
            "--map",
            &map_path,
            "--device",
            device_name,
            // `g_sel`'s input rides in as a real initializer (`volatile
            // unsigned char g_sel = G_SEL`): __start clears zero-initialized
            // globals before main (epic-cc#561), which would erase a
            // sim-side seed.
            "-D",
        ])
        .arg(format!("G_SEL={sel}"))
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} sel={sel}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&map_path);
    let sel_addr = map_addr(&map, "g_sel");
    let seen_addr = map_addr(&map, "g_seen");

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
        device::Core::PicBaseline => panic!("stored_fnptr e2e: pic-baseline not implemented"),
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
