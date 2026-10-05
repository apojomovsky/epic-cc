//! epic-cc#844 pipeline coverage: two strings stage through plain-pointer
//! calls (alloc only stages with two or more sharing the buffer) followed
//! by an indexed 48-byte table. The section model must carry the staged
//! routines' post-banking lengths otherwise a later table's window fold
//! runs at an address hundreds of words early and the real base crosses
//! its window (encoder-sim `.str.4` at 0x1AFF, `.str.2` at 0x18D2).
//! This locks the composed behavior end to end: it builds, every table
//! fits its window, both staged copies deliver, and the indexed reads
//! return the table bytes. The failing-before half lives in the isel
//! regression tests, which pin exact section addresses.

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
    "tests/fixtures/const_staged_drift.c"
}

/// Compile the fixture with the real `epic-cc` binary and return the
/// parsed program words plus the observable globals' `--map` addresses.
fn compile_fixture() -> (Vec<u16>, usize, usize, usize, usize) {
    let hex_path =
        std::env::temp_dir().join(format!("const_staged_drift_{}.hex", std::process::id()));
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
        "epic-cc failed on the staged-drift fixture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let g_buf = map_addr(&map, "g_buf");
    let g_probe = map_addr(&map, "g_probe");
    let g_idx = map_addr(&map, "g_idx");
    let g_out = map_addr(&map, "g_out");
    (
        pic14_sim::parse_hex(&produced),
        g_buf,
        g_probe,
        g_idx,
        g_out,
    )
}

#[test]
fn staged_tables_shift_later_base_inside_its_window() {
    let (prog, g_buf, g_probe, g_idx, g_out) = compile_fixture();

    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[g_idx] = 0; // last read wins: tab[(0 + 9) & 47] = 9
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt");
    // The second staged copy wins the shared buffer: the 32 message bytes
    // of msg2 (plus its NUL terminator in the 33rd slot).
    assert_eq!(
        &p.ram()[g_buf..g_buf + 32],
        b"GHIJKLMNOPQRSTUVWXYZ012345ABCDEF",
        "staged msg2 bytes"
    );
    assert_eq!(p.ram()[g_out], 9, "tab[(0 + 9) & 47]");
    assert_eq!(p.ram()[g_probe], b'5', "staged msg[5]");

    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[g_idx] = 40; // last read wins: tab[(40 + 9) & 47] = 33
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt");
    assert_eq!(p.ram()[g_out], 33, "tab[(40 + 9) & 47]");
}
