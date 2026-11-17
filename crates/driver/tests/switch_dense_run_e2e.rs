//! epic-cc#1417: a switch that irparse preserves as a dense run with a
//! residual chain behind it.
//!
//! The preserved run's case target carries a phi, and the residual
//! edge's incoming must move to the residual chain block while the
//! preserved run's edges keep naming the switch block. A rename that
//! moved every incoming, or that refused the mismatch, broke this
//! shape.

use std::process::Command;

/// Compile a fixture for the PIC18 and report whether it compiled.
fn compiles(fixture: &str) -> bool {
    let hex = std::env::temp_dir().join(format!("switch-dense-run-{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o"])
        .arg(&hex)
        .args(["--device", "18F4550"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex);
    assert!(
        out.status.success(),
        "the dense-run switch did not compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    true
}

/// The dense run's preserved edges and the residual chain's edge land
/// on one case target, so the phi behind it holds an incoming for each.
#[test]
fn dense_run_with_residual_compiles() {
    compiles("tests/fixtures/switch_dense_run.c");
}
