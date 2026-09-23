//! epic-cc#561: the zero-initialization contract, pinned as a test.
//!
//! `__start` clears every zero-initialized RAM global with one LFSR-seeded
//! CLRF loop per contiguous run before `main`, so an uninitialized global
//! reads 0 on real silicon, not just in the simulator (whose RAM starts at
//! 0 and whose POR table now also latches the nonzero SFR resets).
//!
//! What this test guards is that the clearing stays TOTAL. Seeding a byte
//! non-zero before the run, the way real silicon may power up, must still
//! read back 0: if a global ever drops out of the clearing set, the seed
//! survives and this fails.

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

/// An uninitialized mutable global is cleared by `__start`, so its first
/// byte reads 0 even without any store in `main`.
#[test]
fn an_uninitialized_global_reads_zero() {
    let (sim, map) = build_and_run("tests/fixtures/zero_ram.c", "18F4550");
    assert!(sim.halted(), "program must halt");
    let out_at = mapped(&map, "out");
    assert_eq!(
        sim.ram()[out_at],
        0,
        "the clearing loop ran before main, so the global reads 0"
    );
}

/// The load-bearing half: seed the byte with a non-zero value BEFORE the
/// program runs, the way real silicon may power up. `__start` clears it,
/// so the program reads 0 rather than the seed.
#[test]
fn an_uninitialized_global_does_not_reflect_a_nonzero_power_on_seed() {
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
        0,
        "the clearing loop erased the power-on seed before main read it"
    );
}
