//! Chained bitmask lanes (epic-cc#763): the bench-bitmask lane shape
//! with a mixed taken/untaken input pattern runs to halt in the
//! simulator with the exact assembled bytes. The allocator folds each
//! chain onto one accumulator slot, so this pins the destructive-update
//! lowering: a clobbered accumulator reads back as a wrong bit here.
//!
//! g_cfg = {1,0,1,1,1,1,0}: txsta = 0x36, rcsta = 0x88, baudcon = 0x09.
//! Runs on both cores: PIC18 lanes via `BSF`, PIC14 through the generic
//! select over the same shared slot.

use std::process::Command;

/// `out_*` RAM addresses, read off the compiler's own `--map` output.
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn run_on(device_name: &str, core: device::Core) {
    let hex_path = std::env::temp_dir().join(format!(
        "bitmask_chain_{device_name}_{}.hex",
        std::process::id()
    ));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/bitmask_chain.c")
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
    let _ = std::fs::remove_file(&hex_path);
    let txsta = map_addr(&map, "out_txsta");
    let rcsta = map_addr(&map, "out_rcsta");
    let baudcon = map_addr(&map, "out_baudcon");

    match core {
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            p.run(200_000);
            assert_eq!(p.ram()[txsta], 0x36, "txsta mismatch on {device_name}");
            assert_eq!(p.ram()[rcsta], 0x88, "rcsta mismatch on {device_name}");
            assert_eq!(p.ram()[baudcon], 0x09, "baudcon mismatch on {device_name}");
            assert!(p.halted());
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            p.run(200_000);
            assert_eq!(p.ram()[txsta], 0x36, "txsta mismatch on {device_name}");
            assert_eq!(p.ram()[rcsta], 0x88, "rcsta mismatch on {device_name}");
            assert_eq!(p.ram()[baudcon], 0x09, "baudcon mismatch on {device_name}");
            assert!(p.halted());
        }
        other => panic!("bitmask_chain: unsupported core {other:?}"),
    }
}

#[test]
fn bitmask_chain_runs_on_p16() {
    run_on("p16f877a", device::Core::Pic14);
}

#[test]
fn bitmask_chain_runs_on_p18() {
    run_on("p18f4550", device::Core::Pic18);
}
