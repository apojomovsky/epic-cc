//! Lane D determinism check (docs/36): compiling the same input twice must
//! produce byte-identical output. This catches a real bug class (hash-map
//! or hash-set iteration order, an unstable sort) that a single-run
//! correctness check cannot see, and matters because the differential-fuzz
//! methodology assumes determinism to begin with.
//!
//! The driver is run twice over the same fixture to two separate hex files;
//! the two outputs must be byte-identical. A representative fixture is
//! used (one that exercises the whole pipeline: globals, banking, a runtime
//! routine, and a const table), so a nondeterministic stage anywhere in the
//! chain surfaces as a diff.

use std::process::Command;

/// Compile `fixture` to `out_hex` with the driver, asserting success.
fn compile(fixture: &str, out_hex: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o", out_hex, "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn compiling_the_same_input_twice_is_byte_identical() {
    let fixture = "tests/fixtures/const_table.c";
    let hex_a = "tests/fixtures/determinism_a.hex";
    let hex_b = "tests/fixtures/determinism_b.hex";

    compile(fixture, hex_a);
    compile(fixture, hex_b);

    let a = std::fs::read_to_string(hex_a).unwrap();
    let b = std::fs::read_to_string(hex_b).unwrap();
    assert_eq!(
        a, b,
        "compiling {fixture} twice must produce byte-identical hex"
    );

    // Clean up the scratch hex files.
    let _ = std::fs::remove_file(hex_a);
    let _ = std::fs::remove_file(hex_b);
}
