//! `--print-include-dir` and `--dump-include-dir` (epic-cc#690): the
//! PlatformIO builder reads the effective header dir for CPPPATH, and the
//! release bundle assembles `include/` from the same constants the driver
//! compiles with.

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(args)
        .output()
        .expect("run epic-cc")
}

#[test]
fn print_include_dir_reports_an_existing_header_dir() {
    let out = run(&["--print-include-dir"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
    let dir = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    assert!(dir.join("epic-cc.h").is_file(), "dir: {dir:?}");
    assert!(dir.join("stdint.h").is_file(), "dir: {dir:?}");
    assert!(dir.join("xc.h").is_file(), "dir: {dir:?}");
}

#[test]
fn dump_include_dir_materializes_every_header() {
    let dir = std::env::temp_dir().join(format!("epic-cc-dump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = run(&["--dump-include-dir", dir.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for name in [
        "epic-cc.h",
        "stdint.h",
        "stdbool.h",
        "stddef.h",
        "string.h",
        "stdlib.h",
        "malloc.h",
        "xc.h",
        "stdarg.h",
        "stdio.h",
        "math.h",
    ] {
        assert!(dir.join(name).is_file(), "missing {name} in {dir:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn dump_include_dir_needs_a_value() {
    let out = run(&["--dump-include-dir"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--dump-include-dir needs a value"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
