//! Dispatch-lane polarity (epic-cc#806): the #769 bit-test collapse
//! emitted `BTFSS` for `eq` lanes, branching ~30 encoder dispatch lanes
//! the wrong way, and the full suite stayed green. This compiles a
//! two-lane flag dispatch (one `eq` lane, one `ne` lane sharing one
//! snapshot, the `epic_dispatch_all_irqs` shape) and runs it in the sim
//! with the flag set and clear, asserting each handler ran exactly when
//! its lane is taken. Either vector fails on the #769 inversion shape
//! by construction: the `eq` lane runs on the wrong vector each way.
//!
//! The dispatch is called from main rather than the ISR: interrupt
//! vectoring is pinned elsewhere (`pic14-sim` mechanics plus the slice
//! e2e), the miscompile class here is lane lowering.

use std::process::Command;

/// `name`'s RAM address, read off the compiler's own `--map` output.
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: once the frames sit below the
/// globals a difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn compile_lane(tag: &str) -> (Vec<u16>, String) {
    let hex_path =
        std::env::temp_dir().join(format!("dispatch_lane_{tag}_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/dispatch_lane.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (pic14_sim::parse_hex(&produced), map)
}

/// Run the dispatch with `flags` preloaded (RAM starts zeroed, so the
/// preload is the only flag state main observes) and return the markers.
fn run_with_flags(prog: &[u16], map: &str, flags: u8) -> (u8, u8) {
    let flags_addr = map_addr(map, "g_flags");
    let eq_addr = map_addr(map, "g_seen_eq");
    let ne_addr = map_addr(map, "g_seen_ne");
    let mut sim = pic14_sim::Pic14::new(prog.to_vec());
    sim.ram_mut()[flags_addr] = flags;
    sim.run(20_000);
    (sim.ram()[eq_addr], sim.ram()[ne_addr])
}

#[test]
fn eq_lane_runs_only_when_its_bit_is_clear() {
    let (prog, map) = compile_lane("eq");
    // Bit 2 clear: the `eq` lane takes; the `ne` lane (bit 3) does not.
    assert_eq!(
        run_with_flags(&prog, &map, 0x00),
        (0xA1, 0x00),
        "eq lane must run when its bit is clear"
    );
    // Bit 2 set: the `eq` lane must not run (nor the `ne` lane).
    assert_eq!(
        run_with_flags(&prog, &map, 0x04),
        (0x00, 0x00),
        "eq lane must not run when its bit is set"
    );
}

#[test]
fn ne_lane_runs_only_when_its_bit_is_set() {
    let (prog, map) = compile_lane("ne");
    // Bit 3 set (bit 2 set too, so the `eq` lane stays quiet).
    assert_eq!(
        run_with_flags(&prog, &map, 0x0C),
        (0x00, 0xB2),
        "ne lane must run when its bit is set"
    );
    // No bits: only the `eq` lane takes.
    assert_eq!(
        run_with_flags(&prog, &map, 0x00),
        (0xA1, 0x00),
        "ne lane must not run when its bit is clear"
    );
}
