//! #459 acceptance: clang-emitted destructor calls on every exit path
//! survive the pipeline unstranded and unduplicated. The fixture's
//! `noinline` effectful dtor keeps two `D2Ev` calls over three exits at
//! `-O1`; this test pins their count in `--emit ir`. Sim-valued proof of
//! what those cleanups compute belongs to #461.

use std::process::Command;

#[test]
fn cpp_dtor_calls_survive_every_exit_path() {
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cpp_raii.cpp",
            "--emit",
            "ir",
            "-o",
            "tests/fixtures/cpp_raii.ir",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = std::fs::read_to_string("tests/fixtures/cpp_raii.ir").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/cpp_raii.ir");
    let dtors = ir
        .lines()
        .filter(|l| l.contains("call void @_ZN9GuardNoteD2Ev"))
        .count();
    assert_eq!(dtors, 2, "both frontend cleanup calls must survive");
}
