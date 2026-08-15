use std::process::Command;
#[test]
fn probe_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/probe.c", "tests/fixtures/probe.hex"])
        .output().expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));
    let hex = std::fs::read_to_string("tests/fixtures/probe.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 5; // in = 5
    p.run(200_000);
    assert_eq!(p.ram()[0x21], 48); // out == 48
    assert!(p.halted());
}
