use std::process::Command;

#[test]
fn places_a_global_and_resolves_config_end_to_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/config_probe.c",
            "-o",
            "tests/fixtures/config_probe.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    // The resolved-config report is on stderr, unconditional per D-4.
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(report.contains("resolved configuration for p16f877a"), "{report}");

    let hex = std::fs::read_to_string("tests/fixtures/config_probe.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(1000);
    assert!(p.halted());
    // `out` was pinned to 0x0021 by EPIC_AT; irparse's placement reading
    // (Task 1) is what makes this address, not alloc's own choice, land.
    assert_eq!(p.ram()[0x0021], 0x2A);
}
