//! The CLI contract at the real binary: final line, exit codes, and the
//! no-panic guarantee on malformed HEX (design 51 section 3).

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc-sim"))
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn temp_hex(name: &str, text: &str) -> String {
    let path = std::env::temp_dir().join(format!("epic-cc-sim-{}-{name}.hex", std::process::id()));
    std::fs::write(&path, text).expect("write temp hex");
    path.display().to_string()
}

#[test]
fn fixture_hex_runs_to_halted_with_exit_zero() {
    let out = binary()
        .arg(fixture("halt.hex"))
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    // movlw 0x02 then SLEEP, pc staying on the SLEEP word.
    assert_eq!(text, "halted after 3 cycles pc=0x0001 w=0x02 status=0x10\n");
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
}

#[test]
fn cycle_cap_stops_a_loop_with_exit_three() {
    let out = binary()
        .args([fixture("loop.hex"), "--cycles".into(), "10".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(3), "exit: {}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("capped after "), "stdout: {text}");
}

#[test]
fn malformed_hex_exits_one_without_panic_output() {
    // A bad checksum fails pre-validation on line 1.
    let path = temp_hex("badsum", ":040000000230630066\n:00000001FF\n");
    let out = binary().arg(&path).output().expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("malformed HEX in "), "stderr: {err}");
    assert!(err.contains(":1"), "stderr: {err}");
    assert!(!err.contains("panic"), "stderr: {err}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);

    let path = temp_hex("garbage", "not a hex file\n");
    let out = binary().arg(&path).output().expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panic"), "stderr: {err}");

    // A short type-04 record passes the checklist but panics the Pic14e
    // parser, so only the catch_unwind boundary can turn it into exit 1.
    let path = temp_hex("shortela", ":00000004FC\n:00000001FF\n");
    let out = binary()
        .args([path, "--device".into(), "p16f1938".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("malformed HEX in "), "stderr: {err}");
    assert!(!err.contains("panic"), "stderr: {err}");
}

#[test]
fn unreadable_hex_exits_one() {
    let out = binary()
        .arg("/nonexistent/nope.hex")
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}

fn temp_stimulus(name: &str, text: &str) -> String {
    let path = std::env::temp_dir().join(format!("epic-cc-sim-{}-{name}.stim", std::process::id()));
    std::fs::write(&path, text).expect("write temp stimulus");
    path.display().to_string()
}

#[test]
fn timed_poke_is_observed_by_the_program() {
    let stim = temp_stimulus("poke", "at 5 poke 0x20 0x01\n");
    let out = binary()
        .args([fixture("poke_poll.hex"), "--stimulus".into(), stim])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("halted after "));
    // The flag --poke form is observed from step one the same way.
    let out = binary()
        .args([
            fixture("poke_poll.hex"),
            "--poke".into(),
            "0x20=0x01".into(),
        ])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
}

#[test]
fn interrupt_stimulus_is_taken_on_a_core_with_the_hook() {
    let stim = temp_stimulus("irq", "at 5 interrupt\n");
    let out = binary()
        .args([fixture("interrupt.hex"), "--stimulus".into(), stim])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("halted after "));
}

#[test]
fn interrupt_on_a_baseline_device_exits_two() {
    let stim = temp_stimulus("irqbase", "at 5 interrupt\n");
    let out = binary()
        .args([
            fixture("halt.hex"),
            "--device".into(),
            "p12f509".into(),
            "--stimulus".into(),
            stim.clone(),
        ])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&stim), "stderr: {err}");
    assert!(err.contains(":1"), "stderr: {err}");
}

#[test]
fn breakpoint_exits_four_with_breakpoint_final_line() {
    let out = binary()
        .args([fixture("loop.hex"), "--break".into(), "0x0".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(4), "exit: {}", out.status);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("breakpoint after 0 cycles "));
}

#[test]
fn out_of_range_pokes_exit_two_from_flags_and_one_from_stimulus() {
    let out = binary()
        .args([fixture("halt.hex"), "--poke".into(), "0x200=0x01".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let out = binary()
        .args([fixture("halt.hex"), "--poke".into(), "0x00=0x01".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let stim = temp_stimulus("badpoke", "at 1 poke 0x200 0x01\n");
    let out = binary()
        .args([fixture("halt.hex"), "--stimulus".into(), stim.clone()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&stim) && err.contains(":1"), "stderr: {err}");
}

#[test]
fn malformed_stimulus_exits_one_naming_file_and_line() {
    let stim = temp_stimulus("badline", "# ok\nat noon poke 0x20 0x01\n");
    let out = binary()
        .args([fixture("halt.hex"), "--stimulus".into(), stim.clone()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(1), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&stim) && err.contains(":2"), "stderr: {err}");
    assert!(out.stdout.is_empty());
}

#[test]
fn help_and_version_exit_zero_on_stdout() {
    let out = binary().arg("--help").output().expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n", epic_cc_sim::USAGE)
    );
    let out = binary().arg("--version").output().expect("run binary");
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("epic-cc-sim "), "version: {text:?}");
}

#[test]
fn usage_errors_exit_two_with_usage_on_stderr() {
    let out = binary().output().expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("missing <hex>"), "stderr: {err}");
    assert!(err.contains(epic_cc_sim::USAGE), "stderr: {err}");

    let out = binary()
        .args([fixture("halt.hex"), "--devce".into(), "x".into()])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown option --devce"), "stderr: {err}");
    assert!(err.contains("--device"), "stderr: {err}");
    assert!(err.contains(epic_cc_sim::USAGE), "stderr: {err}");

    // Design 51 section 3: usage text follows the diagnostic on stderr.
    let out = binary()
        .args([
            fixture("halt.hex"),
            "--device".into(),
            "no-such-part".into(),
        ])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    let diag = err.find("unknown device no-such-part").expect("diagnostic");
    let usage = err.find(epic_cc_sim::USAGE).expect("usage text");
    assert!(diag < usage, "stderr: {err}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}
