//! An asm operand naming a flash const over 255 bytes (epic-cc#1115).
//! Alloc stages a RAM copy only for consts of 255 bytes or fewer, so the
//! operand has no address to substitute. The driver must reject it with a
//! user error naming the const, not an internal compiler error.

use std::process::Command;

/// A flash const over 255 bytes named by an asm operand has no RAM copy to
/// serve the substitution. The driver must reject it with a user error
/// that names the const, not an internal compiler error.
#[test]
fn asm_operand_over_255_byte_flash_const_is_user_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(std::env::temp_dir().join(format!("asm_flash_const_long_{}.hex", std::process::id())))
        .arg("tests/fixtures/asm_flash_const_long.c")
        .output()
        .expect("run driver");
    assert!(
        !out.status.success(),
        "over-255-byte flash const in an asm operand must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("epic-cc: error: flash const") && stderr.contains("over the 255-byte"),
        "expected a user error naming the const, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("internal compiler error"),
        "must not be an ICE:\n{stderr}"
    );
}
