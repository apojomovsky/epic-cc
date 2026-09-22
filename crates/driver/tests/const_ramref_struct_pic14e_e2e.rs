//! epic-cc#451 acceptance, driver level: the register-map fixture through
//! the whole 16F1937 pipeline (clang -> irparse -> ... -> isel-pic14e ->
//! asm -> HEX). Mirrors the pic18 twin (const_ramref_struct_e2e.rs); the
//! isel-pic14e unit tests cover the emitters directly, this covers the
//! wiring: clang must record the refs and the driver must route them to
//! the materializing emitters.
use std::process::Command;

fn compile_fixture(emit: &str, out_path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F1937", "--emit", emit, "-o"])
        .arg(out_path)
        .arg("tests/fixtures/const_ramref_struct_pic14e.c")
        .output()
        .expect("run epic-cc")
}

#[test]
fn const_ramref_struct_compiles_to_hex_pic14e() {
    let tag = std::process::id();
    let hex_path = std::env::temp_dir().join(format!("const_ramref_451_{tag}.hex"));
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
fn const_ramref_struct_table_materializes_numeric_bytes_pic14e() {
    let tag = std::process::id();
    let asm_path = std::env::temp_dir().join(format!("const_ramref_451_{tag}.asm"));
    let out = compile_fixture("asm", &asm_path);
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).expect("read emitted asm");
    let _ = std::fs::remove_file(&asm_path);

    // holding_regs is a RAM global: it has no assembler label at all,
    // so its name must appear nowhere in the emitted text. `map` is a
    // flash const table and DOES get a label (the init and the table
    // reader resolve LOW(map)/HIGH(map) against it); that is fine.
    assert!(
        !asm.contains("holding_regs"),
        "a RAM global must never be materialized as a label literal:\n{asm}"
    );
    // The map table carries the pointer's two address bytes plus the
    // count byte (clang pads the struct to 4 bytes here): every table
    // byte must be a numeric RETLW, none a label.
    let table = asm.split("\nmap:").nth(1).expect("map table label");
    let table = table.split("\nend").next().expect("table before end");
    let byte_lines: Vec<&str> = table
        .lines()
        .filter(|l| l.trim_start().starts_with("RETLW"))
        .collect();
    assert!(
        byte_lines.len() >= 3,
        "two address bytes plus the count byte:\n{asm}"
    );
    assert!(
        byte_lines.iter().all(|l| {
            l.trim_start()
                .strip_prefix("RETLW ")
                .is_some_and(|v| v.starts_with("0x"))
        }),
        "every table byte must be numeric:\n{asm}"
    );
}
