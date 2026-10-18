//! The `--help`/`--version` contract at the real binary: exit 0 with the
//! usage line or version on stdout, nonzero with usage on stderr for bad
//! flags. Unit coverage lives in `parse_args`; this pins the `main`
//! early-return path those tests cannot reach.

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc-gdbserver"))
}

#[test]
fn help_exits_zero_with_usage_on_stdout() {
    let out = binary().arg("--help").output().expect("run binary");
    assert!(out.status.success(), "exit: {}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n", epic_cc_gdbserver::USAGE)
    );
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
}

#[test]
fn version_exits_zero_with_stamp() {
    let out = binary().arg("--version").output().expect("run binary");
    assert!(out.status.success(), "exit: {}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("epic-cc-gdbserver ") && text.trim().len() > "epic-cc-gdbserver ".len(),
        "version: {text:?}"
    );
}

#[test]
fn unknown_option_exits_two_with_hint_and_usage() {
    let out = binary()
        .args(["a.hex", "a.elf", "--port", "1234", "--devce", "p16f877a"])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "exit: {}", out.status);
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("--device"), "stderr: {text}");
    assert!(text.contains(epic_cc_gdbserver::USAGE), "stderr: {text}");
}
