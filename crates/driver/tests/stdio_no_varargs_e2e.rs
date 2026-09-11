//! epic-cc#391 acceptance: stdio without varargs must compile on both
//! cores. Two triggers, one floor: (a) `<stdio.h>` included but `printf`
//! never called, so the injected runtime's variadic `printf` has no call
//! sites; (b) `printf` called with a literal-only format, so no call site
//! passes extra args. Either left a zero-width va region pre-fix and
//! isel panicked with `va_start in non-variadic context printf`.

use std::process::Command;

fn compiles(device_name: &str, fixture: &str) {
    let hex_path = std::env::temp_dir().join(format!(
        "stdio_no_varargs_{device_name}_{}.hex",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", device_name, "-o"])
        .arg(&hex_path)
        .arg(format!("tests/fixtures/{fixture}"))
        .output()
        .expect("run epic-cc");
    let _ = std::fs::remove_file(&hex_path);
    assert!(
        out.status.success(),
        "epic-cc {device_name} {fixture}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn stdio_without_printf_compiles_pic14() {
    compiles("16F877A", "stdio_no_printf.c");
}

#[test]
fn stdio_without_printf_compiles_pic18() {
    compiles("18F4550", "stdio_no_printf.c");
}

#[test]
fn stdio_printf_without_varargs_compiles_pic14() {
    compiles("16F877A", "stdio_printf_no_varargs.c");
}

#[test]
fn stdio_printf_without_varargs_compiles_pic18() {
    compiles("18F4550", "stdio_printf_no_varargs.c");
}
