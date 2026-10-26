//! epic-cc#1417: a for-loop's induction variable can start from a stale
//! overlay slot.
//!
//! The loop header is the target of a `switch` case, so the counter's
//! phi reaches it through the switch's compare chain. The counter's
//! slot is shared with a byte the chain writes, so a backend that
//! drops the counter's zero init counts from that stale byte and copies
//! nothing.

use std::process::Command;

/// Compile a fixture for the PIC18, run it, and return (sim, map text).
fn build_and_run(fixture: &str) -> (pic14_sim::Pic18, String) {
    let stem = format!(
        "{}-{}",
        std::path::Path::new(fixture)
            .file_stem()
            .expect("stem")
            .to_str()
            .expect("utf8"),
        std::process::id()
    );
    let hex = std::env::temp_dir().join(format!("{stem}.hex"));
    let map = std::env::temp_dir().join(format!("{stem}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", "18F4550"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver pic18: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&map).expect("read map");
    let prog = pic14_sim::parse_hex_pic18(&std::fs::read_to_string(&hex).expect("read hex"));
    let _ = std::fs::remove_file(&hex);
    let _ = std::fs::remove_file(&map);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    (p, text)
}

fn mapped(map: &str, name: &str) -> usize {
    map.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("global ")
                .and_then(|r| r.split_once(' '))
                .filter(|(n, _)| *n == name)
                .and_then(|(_, a)| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
        })
        .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
}

/// The switch's case target is the loop header, so the induction
/// variable's zero init must survive the compare chain that reaches
/// it. The copied byte must be `0xA0`, the first byte the loop writes.
#[test]
fn switch_case_loop_counter_starts_at_zero() {
    let (p, map) = build_and_run("tests/fixtures/switch_loop_counter.c");
    assert!(p.halted(), "the program did not halt");
    assert_eq!(
        p.ram()[mapped(&map, "r")],
        0xA0,
        "the loop counted from a stale overlay slot"
    );
}
