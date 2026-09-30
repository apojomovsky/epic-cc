//! epic-cc#114 acceptance: a const struct table in flash read through a
//! ccp_sel-style pointer select with a runtime instance index.
//!
//! Hand-computed expectations (sim sets `inst` before run):
//!   - addrs[0] = { 0x15, 0x16, 0x17, 0x01 }
//!   - addrs[1] = { 0x1B, 0x1C, 0x1D, 0x02 }
//!   - inst = 0: out_* = 0x15/0x16/0x17/0x01 (inlined select path) and
//!     out_*2 = the same (sunk call-return path)
//!   - inst = 1: out_* = 0x1B/0x1C/0x1D/0x02, out_*2 = the same
//! The table stays `const` in source and must land in flash: the alloc map
//! classifies `addrs` as a const global with no RAM address.

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
fn const_ccp_addrs_selects_run_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_ccp_addrs.c",
            "-o",
            "tests/fixtures/const_ccp_addrs.hex",
            "--map",
            "tests/fixtures/const_ccp_addrs.map",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hex = std::fs::read_to_string("tests/fixtures/const_ccp_addrs.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/const_ccp_addrs.map").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/const_ccp_addrs.map");
    let addr = |n: &str| map_addr(&map, n);

    // The table is const (flash): no RAM allocation, classified as const.
    assert!(
        !map.lines().any(|l| l.starts_with("global addrs 0x")),
        "addrs must stay in flash (no RAM address)"
    );
    assert!(
        map.lines().any(|l| l.trim() == "const addrs"),
        "addrs must be classified as a const global"
    );

    let prog = pic14_sim::parse_hex(&hex);

    // inst = 0 -> element 0 (0x15, 0x16, 0x17, 0x01), both the inlined
    // select and the sunk call-return path.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("inst")] = 0;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_cprl")], 0x15, "cprl (inline) inst=0");
    assert_eq!(p.ram()[addr("out_cprh")], 0x16, "cprh (inline) inst=0");
    assert_eq!(p.ram()[addr("out_con")], 0x17, "con (inline) inst=0");
    assert_eq!(p.ram()[addr("out_irq")], 0x01, "irq (inline) inst=0");
    assert_eq!(p.ram()[addr("out_cprl2")], 0x15, "cprl (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_cprh2")], 0x16, "cprh (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_con2")], 0x17, "con (sunk) inst=0");
    assert_eq!(p.ram()[addr("out_irq2")], 0x01, "irq (sunk) inst=0");

    // inst = 1 -> element 1 (0x1B, 0x1C, 0x1D, 0x02). Reset the machine (the
    // first run ended in SLEEP, so a continued run would not re-enter main).
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("inst")] = 1;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_cprl")], 0x1B, "cprl (inline) inst=1");
    assert_eq!(p.ram()[addr("out_cprh")], 0x1C, "cprh (inline) inst=1");
    assert_eq!(p.ram()[addr("out_con")], 0x1D, "con (inline) inst=1");
    assert_eq!(p.ram()[addr("out_irq")], 0x02, "irq (inline) inst=1");
    assert_eq!(p.ram()[addr("out_cprl2")], 0x1B, "cprl (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_cprh2")], 0x1C, "cprh (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_con2")], 0x1D, "con (sunk) inst=1");
    assert_eq!(p.ram()[addr("out_irq2")], 0x02, "irq (sunk) inst=1");
}
