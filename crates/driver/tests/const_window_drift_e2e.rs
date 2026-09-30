//! epic-cc#151 regression: the const-table section start must be measured
//! at the assembler's final position, not estimated.
//!
//! The section-start estimate used to run banking over the code-only text.
//! The banking pass's CALL-exit-bank analysis needs the const-reader
//! regions (their bodies plus the RETLW table data, which follow the
//! code): measured without them, every reader CALL leaves the tracked
//! bank UNKNOWN and banking over-inserts BANKSEL pairs, over-estimating
//! the code end by a few words. When the real end lands in the top of a
//! 256-byte window (the over-estimate wraps into the next window and
//! looks like a fit), the config table's `.table` base crosses its window
//! and the assembler's window assert panics.
//!
//! Fixture: `const_window_drift.c`. Fifteen small `if (g_idx == K)` blocks
//! each do a const-table read (a `__read_t1` CALL) followed by a banked
//! store, the exact context where the unresolved reader call
//! over-inserts a BANKSEL pair; a sixteenth fat block of fourteen more
//! read+store statements lands the real code end at LOW 0xE8, inside the
//! window's top 24 bytes, with the over-estimate wrapping into the next
//! window. Before the fix the driver panicked at assembly (`const table
//! __epic_config of 69 bytes at base 0x3E8 crosses its 256-byte window`);
//! after the fix it assembles and the sim runs.

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

fn fixture() -> &'static str {
    "tests/fixtures/const_window_drift.c"
}

/// Compile the fixture with the real `epic-cc` binary and return the
/// parsed program words plus the observable globals' `--map` addresses.
fn compile_fixture() -> (Vec<u16>, usize, usize) {
    let hex_path =
        std::env::temp_dir().join(format!("const_window_drift_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-I", "tests/fixtures"])
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture())
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed on the const-window fixture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let g_idx = map_addr(&map, "g_idx");
    let g_out = map_addr(&map, "g_out");
    (pic14_sim::parse_hex(&produced), g_idx, g_out)
}

#[test]
fn config_table_near_window_top_assembles_and_runs() {
    let (prog, g_idx, g_out) = compile_fixture();

    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[g_idx] = 0; // small blocks: only block 0 runs
    p.run(200_000);
    // Block 0: g_out = t1[(0 + 0) & 7] + 0 = t1[0] = 1.
    assert_eq!(p.ram()[g_out], 1, "block 0 store");

    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[g_idx] = 15; // the fat block runs
    p.run(200_000);
    // Fat block's last statement: g_out = t1[(15 + 13) & 7] + 13 = t1[4] + 13.
    assert_eq!(p.ram()[g_out], 5 + 13, "fat block last store");
}
