//! #461 negatives: the existing recursion ban and the heap-`new`
//! subset rejection still fire on C++ input, naming the rule.

use std::process::Command;

fn expect_failure(fixture: &str, needle: &str) {
    let hex_name = format!("{fixture}.hex");
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o", &hex_name, "--device", "p18f4550"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex_name);
    assert!(!output.status.success(), "{fixture} must not compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(needle), "names the rule: {stderr}");
}

#[test]
fn cpp_recursion_still_rejected() {
    expect_failure(
        "tests/fixtures/cpp_recursion_reject.cpp",
        "recursion detected",
    );
}

#[test]
fn cpp_heap_new_still_rejected() {
    expect_failure(
        "tests/fixtures/cpp_new_reject.cpp",
        "heap new/delete are not in the EC++ subset",
    );
}
