//! Cross-context dispatch (the epic-taskmgr shape under epic-cc,
//! epic-hal#86): a task table whose fn field is stored by main while
//! the ISR reads only the flags/countdown fields. Before the
//! field-sensitive ISR-read fix, the whole-table read pulled the stored
//! task into the ISR context, the main dispatch lost its candidate and
//! the fn call trapped; before the width filter, the 1-arg i8 RB site
//! collected the 1-arg ptr-param task and isel panicked copying a
//! narrow arg into a 2-byte slot.

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

#[test]
fn cross_context_dispatch_runs_the_task() {
    let hex_path = std::env::temp_dir().join(format!("cb_dispatch_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/cb_dispatch.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let _ = std::fs::remove_file(&hex_path);
    let seen_addr = map_addr(&map, "g_seen");
    let rb_addr = map_addr(&map, "g_rb");
    let prog = pic14_sim::parse_hex(&produced);
    let mut sim = pic14_sim::Pic14::new(prog);
    sim.run(100_000);
    assert_eq!(
        sim.ram()[seen_addr],
        42,
        "task ran with its arg (main dispatch)"
    );
    assert_eq!(
        sim.ram()[rb_addr],
        0xAB,
        "RB callback site dispatched the i8 callback"
    );
}
