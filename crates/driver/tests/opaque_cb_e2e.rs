// epic-cc#137: a callback stored into an ISR-read global through an opaque
// runtime value (a struct-field load through a pointer, the HAL's
// `g_t0_overflow_cb = h->OverflowCallback` shape) cannot be resolved to a
// candidate. The ISR site compiles to a deterministic trap loop instead of
// panicking on the numeric register name (the pre-#137 behavior) or
// silently calling nothing. This test drives the opaque shape through the
// full pipeline to HEX on both devices and asserts the trap loop is
// present in the emitted asm.

use std::process::Command;

fn run_one(device_name: &str) {
    let hex_path = format!("tests/fixtures/opaque_cb_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/opaque_cb.c",
            "-o",
            &hex_path,
            "--device",
            device_name,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&hex_path);

    // The ISR's indirect call site must lower to a deterministic trap
    // loop: a label immediately followed by a GOTO/BRA to itself.
    let asm_path = format!("tests/fixtures/opaque_cb_{device_name}.asm");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/opaque_cb.c",
            "--emit",
            "asm",
            "-o",
            &asm_path,
            "--device",
            device_name,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} asm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).unwrap();
    let lines: Vec<&str> = asm.lines().collect();
    let trap = lines.windows(2).any(|w| {
        let label = w[0].trim_end_matches(':');
        w[1].contains("GOTO") && w[1].contains(label)
            || w[1].contains("BRA") && w[1].contains(label)
    });
    assert!(
        trap,
        "{device_name}: the opaque ISR call site must emit a trap loop:\n{asm}"
    );
    let _ = std::fs::remove_file(&asm_path);
}

#[test]
fn opaque_callback_store_compiles_to_hex_on_both_devices() {
    run_one("p16f877a");
    run_one("p18f4550");
}
