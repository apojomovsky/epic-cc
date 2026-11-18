//! T3 contract: `--trace-sfr` change-only tracing and `--dump` output
//! (design 51 D-4 and section 3), at the real binary.

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc-sim"))
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[String]) -> std::process::Output {
    binary().args(args).output().expect("run binary")
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn portb_trace_lines_in_order_on_blink() {
    let out = run(&[
        fixture("blink.hex"),
        "--trace-sfr".into(),
        "PORTB".into(),
        "--cycles".into(),
        "10".into(),
    ]);
    assert_eq!(out.status.code(), Some(3), "exit: {}", out.status);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..4],
        [
            "cycle=2 pc=0x0001 write PORTB=0x01",
            "cycle=4 pc=0x0003 write PORTB=0x00",
            "cycle=8 pc=0x0001 write PORTB=0x01",
            "cycle=10 pc=0x0003 write PORTB=0x00",
        ],
        "stdout: {text}"
    );
    assert!(
        lines[4].starts_with("capped after 10 cycles "),
        "stdout: {text}"
    );
    assert_eq!(lines.len(), 5, "stdout: {text}");
}

#[test]
fn same_value_write_emits_no_line() {
    let out = run(&[
        fixture("samevalue.hex"),
        "--trace-sfr".into(),
        "PORTB".into(),
    ]);
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    let text = stdout(&out);
    let trace: Vec<&str> = text.lines().filter(|l| l.contains(" write ")).collect();
    assert_eq!(
        trace,
        ["cycle=2 pc=0x0001 write PORTB=0x01"],
        "stdout: {text}"
    );
}

#[test]
fn unknown_sfr_name_exits_two() {
    let out = run(&[
        fixture("blink.hex"),
        "--trace-sfr".into(),
        "NOSUCHREG".into(),
    ]);
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("NOSUCHREG"), "stderr: {err}");
    assert!(err.contains(epic_cc_sim::USAGE), "stderr: {err}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}

#[test]
fn indf_trace_exits_two_by_name_and_address() {
    for target in ["INDF", "0x00"] {
        let out = run(&[fixture("blink.hex"), "--trace-sfr".into(), target.into()]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "target {target} exit: {}",
            out.status
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("INDF"), "stderr: {err}");
        assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    }
}

#[test]
fn indf_trace_exits_two_by_address_on_pic14e_pic18_and_baseline() {
    for (device, target) in [
        ("p16f1937", "INDF0"),
        ("p16f1937", "INDF1"),
        ("p16f1937", "0x00"),
        ("p16f1937", "0x01"),
        ("p18f4550", "INDF2"),
        ("p18f4550", "0xFEF"),
        ("p18f4550", "0xFE7"),
        ("p18f4550", "0xFDF"),
        ("p12f509", "0x00"),
    ] {
        let out = run(&[
            fixture("halt.hex"),
            "--device".into(),
            device.into(),
            "--trace-sfr".into(),
            target.into(),
        ]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{device} {target} exit: {}",
            out.status
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("INDF"), "{device} {target} stderr: {err}");
        assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    }
}

#[test]
fn duplicate_trace_specs_emit_one_line_per_change() {
    let out = run(&[
        fixture("samevalue.hex"),
        "--trace-sfr".into(),
        "PORTB,0x06,PORTB".into(),
    ]);
    assert_eq!(out.status.code(), Some(0), "exit: {}", out.status);
    let text = stdout(&out);
    assert_eq!(text.matches("write PORTB=").count(), 1, "stdout: {text}");
}

#[test]
fn dump_formats_match_golden_files() {
    for (what, golden) in [
        ("regs", "dump-regs.golden"),
        ("ram", "dump-ram.golden"),
        ("all", "dump-all.golden"),
    ] {
        let out = run(&[fixture("cell.hex"), "--dump".into(), what.into()]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "dump {what} exit: {}",
            out.status
        );
        let expected = std::fs::read_to_string(fixture(golden)).expect("read golden");
        assert_eq!(stdout(&out), expected, "dump {what}");
    }
}

#[test]
fn bad_dump_value_exits_two() {
    let out = run(&[fixture("cell.hex"), "--dump".into(), "everything".into()]);
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}
