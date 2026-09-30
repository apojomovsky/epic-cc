//! `_delay(cycles)` end to end (epic-cc#700): a C program declaring the
//! intrinsic with no definition must clear wholeprog like an `llvm.*`
//! intrinsic, and the emitted loop must take exactly the requested
//! cycles in the simulator. Each core compiles the delay program and an
//! empty main; the cycle delta is the loop itself, so prologue and
//! epilogue stay out of the assertion.

use std::process::Command;

fn cycles_for(device: &device::Device, device_name: &str, c_file: &str) -> u64 {
    let hex_path = std::env::temp_dir().join(format!(
        "delay_{device_name}_{c_file}_{}.hex",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "-o"])
        .arg(&hex_path)
        .arg(format!("tests/fixtures/{c_file}.c"))
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} {c_file}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);
    let cycles = match device.core {
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            p.run(200_000);
            assert!(p.halted(), "delay program must halt on {device_name}");
            p.cycles()
        }
        device::Core::Pic14e => {
            let mut p = pic14_sim::Pic14e::with_device(
                &device::PIC16F1937,
                pic14_sim::parse_hex_pic14e(&hex),
            );
            p.run(200_000);
            assert!(p.halted(), "delay program must halt on {device_name}");
            p.cycles()
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            p.run(200_000);
            assert!(p.halted(), "delay program must halt on {device_name}");
            p.cycles()
        }
        device::Core::PicBaseline => {
            let mut p =
                pic14_sim::PicBaseline::with_device(&device::PIC12F509, pic14_sim::parse_hex(&hex));
            p.run(200_000);
            assert!(p.halted(), "delay program must halt on {device_name}");
            p.cycles()
        }
    };
    cycles
}

fn delay_is_exact_on(device: &device::Device, device_name: &str) {
    let base = cycles_for(device, device_name, "delay_empty");
    assert_eq!(
        cycles_for(device, device_name, "delay") - base,
        100,
        "_delay(100) must take exactly 100 cycles on {device_name}"
    );
}

#[test]
fn delay_is_exact_on_pic14() {
    delay_is_exact_on(&device::PIC16F877A, "p16f877a");
}

#[test]
fn delay_is_exact_on_pic14_no_common_ram() {
    // docs/39 bucket 2 (F74): counters in banked `isr_home_window`.
    delay_is_exact_on(&device::PIC16F74, "p16f74");
}

#[test]
fn delay_is_exact_on_pic14_no_common_ram_bucket1() {
    // docs/39 bucket 1 (873A): same pin, other bucket.
    delay_is_exact_on(&device::PIC16F873A, "p16f873a");
}

/// Final asm for a fixture on a no-common-RAM part, to guard the
/// bank-1-traffic precondition below.
fn asm_for(device_name: &str, c_file: &str) -> String {
    let asm_path = std::env::temp_dir().join(format!(
        "delay_{device_name}_{c_file}_{}.asm",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "--emit", "asm", "-o"])
        .arg(&asm_path)
        .arg(format!("tests/fixtures/{c_file}.c"))
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name} {c_file}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).unwrap();
    let _ = std::fs::remove_file(&asm_path);
    asm
}

/// Banking's own selects use the named `STATUS, 5/6` form while the #800
/// pin uses the `0x03` address form, so this only matches selects
/// banking inserted for genuine bank-1 traffic.
fn assert_banked_selects(asm: &str, device_name: &str) {
    assert!(
        asm.lines().any(|l| {
            let t = l.trim();
            t.starts_with("BSF STATUS, 5")
                || t.starts_with("BCF STATUS, 5")
                || t.starts_with("BSF STATUS, 6")
                || t.starts_with("BCF STATUS, 6")
        }),
        "banking must see bank-1 traffic on {device_name} (got none):\n{asm}"
    );
}

fn delay_beside_bank1_traffic_is_exact_on(device: &device::Device, device_name: &str) {
    // epic-cc#800: ga[60] + gb[40] exceed bank 0's 79 bytes, so gb lands
    // in bank 1 on both parts (a single 90-byte array fits nowhere on
    // the 873A) and the delay prologue carries the selects delay-only
    // programs never see. The exact delta proves banking added nothing
    // inside the loop.
    let asm = asm_for(device_name, "delay_bank1");
    assert_banked_selects(&asm, device_name);
    let base = cycles_for(device, device_name, "delay_bank1_empty");
    assert_eq!(
        cycles_for(device, device_name, "delay_bank1") - base,
        100,
        "_delay(100) beside bank-1 traffic must take exactly 100 cycles on {device_name}"
    );
}

#[test]
fn delay_beside_bank1_traffic_on_p16f74() {
    delay_beside_bank1_traffic_is_exact_on(&device::PIC16F74, "p16f74");
}

#[test]
fn delay_beside_bank1_traffic_on_p16f873a() {
    delay_beside_bank1_traffic_is_exact_on(&device::PIC16F873A, "p16f873a");
}

#[test]
fn delay_is_exact_on_pic14e() {
    delay_is_exact_on(&device::PIC16F1937, "p16f1937");
}

#[test]
fn delay_is_exact_on_pic18() {
    delay_is_exact_on(&device::PIC18F4550, "p18f4550");
}

#[test]
fn delay_is_exact_on_pic_baseline() {
    delay_is_exact_on(&device::PIC12F509, "p12f509");
}

#[test]
fn delay_rejects_a_runtime_count() {
    let hex_path = std::env::temp_dir().join(format!("delay_var_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p16f877a", "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/delay_var.c")
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex_path);
    assert!(
        !out.status.success(),
        "a runtime _delay count must fail the build"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("compile-time constant"),
        "the error must name the constant-count rule: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn delay_beside_a_callback_is_exact_on(device: &device::Device, device_name: &str) {
    // An address-taken callback with the same arity and width as `_delay`
    // must not divert the call into indirect dispatch: legalize keeps its
    // candidate list empty and the loop stays exact.
    let base = cycles_for(device, device_name, "delay_cb_empty");
    assert_eq!(
        cycles_for(device, device_name, "delay_cb") - base,
        100,
        "_delay(100) beside a callback must take exactly 100 cycles on {device_name}"
    );
}

#[test]
fn delay_beside_a_callback_on_pic14() {
    delay_beside_a_callback_is_exact_on(&device::PIC16F877A, "p16f877a");
}

#[test]
fn delay_beside_a_callback_on_pic14e() {
    delay_beside_a_callback_is_exact_on(&device::PIC16F1937, "p16f1937");
}

#[test]
fn delay_beside_a_callback_on_pic18() {
    delay_beside_a_callback_is_exact_on(&device::PIC18F4550, "p18f4550");
}

#[test]
fn delay_beside_a_callback_on_pic_baseline() {
    delay_beside_a_callback_is_exact_on(&device::PIC12F509, "p12f509");
}
