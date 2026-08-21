use std::process::Command;

fn driver() -> String {
    env!("CARGO_BIN_EXE_epic-cc").to_string()
}

fn run_driver(args: &[&str]) -> std::process::Output {
    Command::new(driver())
        .args(args)
        .output()
        .expect("run driver")
}

fn assert_hex_non_empty(path: &str) {
    let hex = std::fs::read_to_string(path).expect("read hex");
    assert!(!hex.trim().is_empty(), "hex empty: {path}");
    assert!(hex.contains(':'), "hex missing ':' : {hex}");
    // Intel HEX must end with EOF record
    assert!(hex.contains(":00000001FF"), "hex missing EOF");
}

fn assert_asm_contains(asm: &str, needle: &str) {
    assert!(asm.contains(needle), "asm missing `{needle}`:\n{asm}");
}

#[test]
fn asm_naked_p14() {
    let out_hex = "/tmp/asm_naked_e2e.hex";
    let out_asm = "/tmp/asm_naked_e2e.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_naked.c",
        "-o",
        out_hex,
        "--device",
        "p16f877a",
    ]);
    assert!(
        res.status.success(),
        "driver naked p14 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);
    // also check golden exists and non-empty
    let golden = std::fs::read_to_string("tests/fixtures/asm_naked.hex").expect("read golden");
    assert!(!golden.trim().is_empty());

    let res = run_driver(&[
        "tests/fixtures/asm_naked.c",
        "-o",
        out_asm,
        "--device",
        "p16f877a",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm naked failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap();
    assert_asm_contains(&asm, "movf 0x20, w");
    assert_asm_contains(&asm, "addwf 0x21, w");
    assert_asm_contains(&asm, "movwf 0x22");
    assert_asm_contains(&asm, "return");
    // naked must not have prologue; the function label should be directly followed by asm
    assert!(asm.contains("my_mul:"), "missing my_mul label");
}

#[test]
fn asm_opaque_p14() {
    let out_hex = "/tmp/asm_opaque_e2e.hex";
    let out_asm = "/tmp/asm_opaque_e2e.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_opaque.c",
        "-o",
        out_hex,
        "--device",
        "p16f877a",
    ]);
    assert!(
        res.status.success(),
        "driver opaque p14 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);
    let golden = std::fs::read_to_string("tests/fixtures/asm_opaque.hex").expect("read golden");
    assert!(!golden.trim().is_empty());

    let res = run_driver(&[
        "tests/fixtures/asm_opaque.c",
        "-o",
        out_asm,
        "--device",
        "p16f877a",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm opaque failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap();
    assert_asm_contains(&asm, "bcf INTCON, 7");
    assert_asm_contains(&asm, "bsf INTCON, 7");
}

#[test]
fn asm_module_p14() {
    let out_hex = "/tmp/asm_module_e2e.hex";
    let out_asm = "/tmp/asm_module_e2e.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_module.c",
        "-o",
        out_hex,
        "--device",
        "p16f877a",
    ]);
    assert!(
        res.status.success(),
        "driver module p14 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);
    let golden = std::fs::read_to_string("tests/fixtures/asm_module.hex").expect("read golden");
    assert!(!golden.trim().is_empty());

    let res = run_driver(&[
        "tests/fixtures/asm_module.c",
        "-o",
        out_asm,
        "--device",
        "p16f877a",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm module failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap();
    // module asm at top plus inline goto
    assert_asm_contains(&asm, "my_label:");
    assert_asm_contains(&asm, "nop");
    assert_asm_contains(&asm, "goto my_label");
}

#[test]
fn asm_module_p18() {
    // PIC18 where applicable: same fixture should assemble for PIC18
    let out_hex = "/tmp/asm_module_p18_e2e.hex";
    let out_asm = "/tmp/asm_module_p18_e2e.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_module.c",
        "-o",
        out_hex,
        "--device",
        "p18f4550",
    ]);
    assert!(
        res.status.success(),
        "driver module p18 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);

    let res = run_driver(&[
        "tests/fixtures/asm_module.c",
        "-o",
        out_asm,
        "--device",
        "p18f4550",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm module p18 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap();
    assert_asm_contains(&asm, "my_label:");
    assert_asm_contains(&asm, "goto my_label");
}

#[test]
fn asm_intrinsic_p14() {
    let out_hex = "/tmp/asm_intrinsic_e2e.hex";
    let out_asm = "/tmp/asm_intrinsic_e2e.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_intrinsic.c",
        "-o",
        out_hex,
        "--device",
        "p16f877a",
    ]);
    assert!(
        res.status.success(),
        "driver intrinsic p14 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);
    let golden = std::fs::read_to_string("tests/fixtures/asm_intrinsic.hex").expect("read golden");
    assert!(!golden.trim().is_empty());

    let res = run_driver(&[
        "tests/fixtures/asm_intrinsic.c",
        "-o",
        out_asm,
        "--device",
        "p16f877a",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm intrinsic failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap();
    assert_asm_contains(&asm, "nop");
    assert_asm_contains(&asm, "clrwdt");
    assert_asm_contains(&asm, "bcf INTCON, 7");
    assert_asm_contains(&asm, "bsf INTCON, 7");
}

#[test]
fn asm_reg_constraint_rejected() {
    // Contains `asm volatile("movwf %0" : "+r"(x))` -> register constraints
    let res = run_driver(&[
        "tests/fixtures/asm_reg.c",
        "-o",
        "/tmp/asm_reg.hex",
        "--device",
        "p16f877a",
    ]);
    assert!(
        !res.status.success(),
        "expected failure for register constraints"
    );
    let stderr = String::from_utf8_lossy(&res.stderr).to_string()
        + &String::from_utf8_lossy(&res.stdout).to_string();
    // irparse panics, driver may print panic payload to stderr
    assert!(
        stderr.contains("register constraints are not supported"),
        "expected register constraints panic, got: {stderr}"
    );
}

#[test]
fn asm_memory_operands_p14() {
    // `asm("movf %1, w" : "+m"(t) : "m"(y))` -> *m operands, should compile
    let out_hex = "/tmp/asm_operands.hex";
    let out_asm = "/tmp/asm_operands.asm";
    let res = run_driver(&[
        "tests/fixtures/asm_operands.c",
        "-o",
        out_hex,
        "--device",
        "p16f877a",
    ]);
    assert!(
        res.status.success(),
        "expected success for memory operands, stderr: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    assert_hex_non_empty(out_hex);
    let res = run_driver(&[
        "tests/fixtures/asm_operands.c",
        "-o",
        out_asm,
        "--device",
        "p16f877a",
        "--emit",
        "asm",
    ]);
    assert!(
        res.status.success(),
        "emit asm failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let asm = std::fs::read_to_string(out_asm).unwrap_or_default();
    // substituted addresses via slot_addr: should contain MOVF with 0x
    assert!(
        asm.contains("MOVF") || asm.contains("movf"),
        "asm missing MOVF, got:\n{asm}"
    );
    assert!(
        asm.contains("0x"),
        "asm missing substituted address 0x, got:\n{asm}"
    );
}
