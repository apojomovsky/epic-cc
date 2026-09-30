//! Milestone-3 overlay acceptance: two sibling functions (big_a, big_b) each
//! carry >= 16 bytes of simultaneous live i16 locals, called sequentially
//! from main. Acceptance: (a) the program runs correctly in the simulator,
//! (b) the local address map shows big_a and big_b sharing a base region
//! (overlay), and total_bank0 < locals_size(big_a) + locals_size(big_b) +
//! locals_size(main).

use std::process::Command;

/// Sorted RAM addresses of one function's frame locals, read off the
/// compiler's own `--map` output (`local {func}::{name} 0xNN` lines).
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts (see array_e2e's `map_addr`).
fn frame_addrs(map: &str, fname: &str) -> Vec<u16> {
    let prefix = format!("local {fname}::");
    let mut addrs: Vec<u16> = map
        .lines()
        .filter(|l| l.starts_with(&prefix))
        .map(|l| {
            let addr = l.rsplit(' ').next().expect("map local has an address");
            u16::from_str_radix(addr.trim_start_matches("0x"), 16).expect("map address is hex")
        })
        .collect();
    assert!(!addrs.is_empty(), "function {fname} has map locals");
    addrs.sort();
    addrs
}

/// Widthless lower bound on a frame's span: max(addr) - min(addr) + 1.
/// Every local is at least 1 byte, so the true span (which adds each
/// value's width) only grows from here. Sound for the >= 16 guards: a
/// folded-away fixture collapses to ~2, far below 16, so the bound
/// cannot pass a broken fixture.
fn span_lo(addrs: &[u16]) -> u16 {
    addrs[addrs.len() - 1] - addrs[0] + 1
}

/// One alloc scalar off the `--map` output (see size_map_e2e).
fn map_scalar(map: &str, kind: &str) -> u16 {
    let prefix = format!("{kind} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {kind} in:\n{map}"));
    u16::from_str_radix(line[prefix.len()..].trim(), 16).expect("map scalar is hex")
}

/// Run the driver on the overlay fixture for its HEX and map.
fn build_overlay() -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/overlay.c",
            "-o",
            "tests/fixtures/overlay.hex",
            "--device",
            "p16f877a",
            "--map",
            "tests/fixtures/overlay.map",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string("tests/fixtures/overlay.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/overlay.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/overlay.map");
    (hex, map)
}

#[test]
fn overlay_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/overlay.c",
            "-o",
            "tests/fixtures/overlay.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/overlay.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 3; // in = 3
    p.run(500_000);
    // big_a(3) = 3+4+..+10 = 52; big_b(in+1=4) = 0+1+2+3+5+6+7+8 = 32;
    // out = (unsigned char)(52 + 32) = 84
    assert_eq!(p.ram()[0x21], 84);
    assert!(p.halted());
}

#[test]
fn overlay_frames_share_ram() {
    let (_hex, map) = build_overlay();
    let (a, b, m) = (
        frame_addrs(&map, "big_a"),
        frame_addrs(&map, "big_b"),
        frame_addrs(&map, "main"),
    );

    // The critical .ll property: each sibling carries >= 16 bytes of
    // simultaneous i16 locals (else -O1 folded the program away).
    let (span_a, span_b, span_main) = (span_lo(&a), span_lo(&b), span_lo(&m));
    assert!(span_a >= 16 && span_b >= 16,
        "each sibling must carry >= 16 bytes of simultaneous locals (got big_a={span_a}, big_b={span_b})");

    // (b) sibling frames overlay: identical base region (never co-live).
    assert_eq!(a[0], b[0], "big_a and big_b must share a base address");

    // main's frame is disjoint and sits before the shared sibling region.
    // main's values are i8/i16 (the fixture has no wider type), so its
    // true span exceeds the widthless bound by at most 1.
    assert!(m[0] + span_main + 1 <= a[0]);

    // Overlay wins: total bank-0 demand < sum of the three demands.
    // Lower-bound spans only shrink the sum, so passing against them
    // implies passing against the true spans.
    let sum_demands = span_a + span_b + span_main;
    let total_bank0 = map_scalar(&map, "total-bank0");
    assert!(
        total_bank0 < sum_demands,
        "total_bank0 {total_bank0} must be < sum of individual demands {sum_demands}"
    );
}
