use std::process::Command;

#[test]
fn compiles_straight_line_program_end_to_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/add.c",
            "-o",
            "tests/fixtures/add.hex",
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
    // Simulate the output and assert out == in + 1
    let hex = std::fs::read_to_string("tests/fixtures/add.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 0x07; // in = 7
    p.run(1000);
    assert_eq!(p.ram()[0x21], 0x08); // out = 8
    assert!(p.halted());
}
