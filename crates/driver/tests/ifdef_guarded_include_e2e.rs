//! epic-cc#196 acceptance: a source that guards `#include <stdio.h>`
//! behind `#ifndef __EPIC_CC__` (epic-hal's own pattern for staying
//! stdio-free under epic-cc) must build cleanly when `__EPIC_CC__` is
//! defined, even though the file never provides a `putchar`. Before the
//! fix, `need_stdio` grepped the raw source text and injected the stdio
//! runtime regardless of the guard, and that runtime's `putchar` call
//! would fail to link.

use std::process::Command;

#[test]
fn guarded_stdio_include_is_not_injected_when_the_guard_excludes_it() {
    let hex_path =
        std::env::temp_dir().join(format!("ifdef_guarded_stdio_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/ifdef_guarded_stdio.c",
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "-D",
            "__EPIC_CC__",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex_path);
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
