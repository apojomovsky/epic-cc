//! Asm operands naming flash consts (epic-cc#1115, epic-cc#1132). Alloc
//! stages a RAM copy for consts of 255 bytes or fewer, so a small const
//! named only by asm compiles with its copy in RAM, while a larger one
//! has no address to substitute and the driver rejects it with a user
//! error naming the const, never an internal compiler error.

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

/// A small flash const named only by an asm operand keeps a RAM copy for
/// the substitution (epic-cc#1132). The driver must compile it cleanly
/// and place the const in RAM, never ICE with no address.
#[test]
fn asm_operand_small_flash_const_compiles_with_ram_copy() {
    let dir = std::env::temp_dir();
    let tag = std::process::id();
    let hex = dir.join(format!("asm_flash_const_small_{tag}.hex"));
    let map = dir.join(format!("asm_flash_const_small_{tag}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .arg("tests/fixtures/asm_flash_const_small.c")
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "small flash const in an asm operand must compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("internal compiler error"),
        "must not be an ICE:\n{stderr}"
    );
    let hex_text = std::fs::read_to_string(&hex).expect("read hex");
    assert!(
        hex_text.contains(":00000001FF"),
        "hex must end with the EOF record"
    );
    let map_text = std::fs::read_to_string(&map).expect("read map");
    assert!(
        map_text.lines().any(|l| l.starts_with("global SMALL 0x")),
        "asm-named const SMALL must hold a RAM address, got:\n{map_text}"
    );
}
