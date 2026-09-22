//! epic-cc#465: an i1 global lowered by PIC14's `isel`. clang's own -O1
//! GlobalOpt narrows an internal flag only ever written 0/1 down to
//! `global i1` (epic-cc#462), so this is a shape real programs produce.
//! `isel` used to panic with "only i8/i16 loads supported"; PIC18 already
//! handled it (epic-cc#464).
//!
//! The flag representation is inverted, `@flag = global i1 false` with
//! `set()` storing false and `clear()` storing true, and store-to-load
//! forwarding folds the trailing `if (flag)` away, so `main` is a load, a
//! select and two stores. `out == 1` follows from the load reading 0x00.

use std::process::Command;

#[test]
fn i1_memory_lowers_and_runs_on_pic16() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/i1_memory.c",
            "-o",
            "tests/fixtures/i1_memory_16F877A.hex",
            "--map",
            "tests/fixtures/i1_memory_16F877A.map",
            "--device",
            "16F877A",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = std::fs::read_to_string("tests/fixtures/i1_memory_16F877A.map").unwrap();
    let addr = |name: &str| -> usize {
        map.lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("global ")
                    .and_then(|r| r.split_once(' '))
                    .filter(|(n, _)| *n == name)
                    .and_then(|(_, a)| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            })
            .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
    };
    let hex = std::fs::read_to_string("tests/fixtures/i1_memory_16F877A.hex").unwrap();
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(200_000);
    assert!(p.halted(), "must halt");
    // The inverted flag loads as 0x00, so the select picks 1 (see the
    // header). A backend that failed to lower the i1 load would not reach
    // here at all: it panics, as master does.
    assert_eq!(
        p.ram()[addr("out")],
        1,
        "i1 load must lower and feed the select"
    );
    // `main` ends with `store i1 true, ptr @flag`, so the flag byte must be
    // exactly 1 at halt. This is the end-to-end check that an i1 STORE
    // writes the normalized 0/1 convention rather than some wider truthy
    // value (0xFF, a full byte copy of the source): every i1 consumer tests
    // the whole byte for nonzero, so the convention is load-bearing.
    assert_eq!(
        p.ram()[addr("flag")],
        1,
        "an i1 store must write 0/1, not an arbitrary truthy byte"
    );
}
