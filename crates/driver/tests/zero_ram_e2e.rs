//! epic-cc#557: the zero-initialization contract, pinned as a test.
//!
//! RAM-resident globals are not cleared at startup: no `__start` code writes
//! zeros, and the convention everywhere is that RAM reads zero before any
//! write. That holds in the simulator (RAM starts at 0) but NOT on real
//! silicon, where PIC RAM is indeterminate at power-on.
//!
//! The cost of changing it is measured, not assumed. On the epic menu-demo
//! fixture there are 43 zero-initialized mutable globals totalling about
//! 520-522 bytes (the figure depends on how struct padding is counted), so
//! clearing them costs on the order of a thousand flash words on an
//! 11831-word program, roughly +9%. The decision is therefore deliberate:
//! keep relying on zeroed RAM until there is a reason to pay for it. Filing
//! the clearing work is epic-cc#561.
//!
//! What this test guards is that the reliance stays VISIBLE. If someone later
//! emits a clearing loop, this test fails and must be rewritten, which is the
//! point: the assumption should not be able to change silently.

use std::process::Command;

/// Compile a fixture for `device`, run it, and return (sim, map text).
fn build_and_run(fixture: &str, device: &str) -> (pic14_sim::Pic18, String) {
    let stem = format!(
        "{}_{}",
        std::path::Path::new(fixture)
            .file_stem()
            .expect("stem")
            .to_str()
            .expect("utf8"),
        device
    );
    let hex = std::env::temp_dir().join(format!("{stem}-{}.hex", std::process::id()));
    let map = std::env::temp_dir().join(format!("{stem}-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", device])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&map).expect("read map");
    let prog = pic14_sim::parse_hex_pic18(&std::fs::read_to_string(&hex).expect("read hex"));
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

/// An uninitialized mutable global is NEVER written, so `__start` contains no
/// store for it and its first byte holds whatever RAM was seeded with. The
/// simulator seeds zero, which is why the program still reads 0 here.
#[test]
fn an_uninitialized_global_gets_no_init_code_and_reads_the_ram_seed() {
    let (sim, map) = build_and_run("tests/fixtures/zero_ram.c", "18F4550");
    assert!(sim.halted(), "program must halt");
    let buf = mapped(&map, "buf");
    assert_eq!(
        sim.ram()[buf],
        0,
        "the sim seeds RAM with zero, so the uninitialized global reads 0"
    );
}

/// The load-bearing half: seed the byte with a non-zero value BEFORE the
/// program runs, the way real silicon may power up. Nothing in `__start`
/// clears it, so the program reads the seed straight back. This is the
/// hazard the reliance above creates, made observable.
#[test]
fn an_uninitialized_global_reflects_a_nonzero_power_on_seed() {
    let hex = std::env::temp_dir().join(format!("zero_ram_seed-{}.hex", std::process::id()));
    let map = std::env::temp_dir().join(format!("zero_ram_seed-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/zero_ram.c", "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", "18F4550"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&map).expect("read map");
    let prog = pic14_sim::parse_hex_pic18(&std::fs::read_to_string(&hex).expect("read hex"));
    let mut p = pic14_sim::Pic18::new(prog);
    let buf = mapped(&text, "buf");
    let out_at = mapped(&text, "out");
    // Power-on seed, as real hardware may supply.
    p.ram_mut()[buf] = 0xAB;
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[out_at],
        0xAB,
        "no init code clears the global, so a non-zero power-on seed survives \
         into the program: this is why the zero-RAM reliance is documented \
         rather than assumed away"
    );
}
