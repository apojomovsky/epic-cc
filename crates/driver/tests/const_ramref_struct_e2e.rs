//! epic-cc#443 acceptance: a `static const` struct whose pointer field
//! holds a RAM global's ADDRESS (epic-hal combo-modbus-full's register
//! map) must compile through to HEX. The const table's refs name a RAM
//! global, which has no assembler label, so the emitted bytes are the
//! alloc-time address; before the fix the assembler panicked with
//! "asm: LOW(holding_regs) label not found".
use std::process::Command;

fn compile_fixture(emit: &str, out_path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "18F4550", "--emit", emit, "-o"])
        .arg(out_path)
        .arg("tests/fixtures/const_ramref_struct_pic18.c")
        .output()
        .expect("run epic-cc")
}

#[test]
fn const_ramref_struct_compiles_to_hex() {
    let tag = std::process::id();
    let hex_path = std::env::temp_dir().join(format!("const_ramref_443_{tag}.hex"));
    let out = compile_fixture("hex", &hex_path);
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read_to_string(&hex_path).is_ok(),
        "hex file must exist"
    );
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn const_ramref_struct_table_materializes_numeric_bytes() {
    let tag = std::process::id();
    let asm_path = std::env::temp_dir().join(format!("const_ramref_443_{tag}.asm"));
    let out = compile_fixture("asm", &asm_path);
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).expect("read emitted asm");
    let _ = std::fs::remove_file(&asm_path);

    assert!(
        !asm.contains("LOW(holding_regs)") && !asm.contains("HIGH(holding_regs)"),
        "a RAM global must never be materialized as a label literal:\n{asm}"
    );
    // The map table carries the pointer's two address bytes plus the
    // count byte, each numeric.
    let table = asm.split("map:").nth(1).expect("map table label");
    let table = table.split("\nend").next().expect("table before end");
    assert!(
        !table.contains("LOW(") && !table.contains("HIGH("),
        "table bytes must be numeric:\n{asm}"
    );
    assert_eq!(
        table.matches("db 0x").count(),
        3,
        "two address bytes plus the count byte:\n{asm}"
    );
}
