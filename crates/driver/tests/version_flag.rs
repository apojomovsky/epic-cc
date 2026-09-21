//! `--version` prints the compiler identity a downstream job can pin and
//! report. The stamp comes from the driver build.rs: EPIC_CC_VERSION when set
//! (the docker release stage's build ARG), the crate version plus the
//! checkout's commit otherwise.
//!
//! The tests assert the identity a consumer depends on: epic-hal's guard
//! compares this sha against its epic-cc checkout's HEAD to tell a stale
//! driver from a current one (epic-hal#240, #260).

use std::process::Command;

/// The `--version` stamp, with the "epic-cc " prefix stripped.
fn stamp() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg("--version")
        .output()
        .expect("run epic-cc --version");
    assert!(out.status.success(), "exit code: {:?}", out.status);
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().expect("--version prints a line");
    assert!(
        line.starts_with("epic-cc "),
        "expected 'epic-cc <stamp>', got: {line:?}"
    );
    let stamp = line.trim_start_matches("epic-cc ").to_string();
    assert!(!stamp.is_empty(), "stamp must not be empty");
    stamp
}

/// The commit of the checkout `cargo` is building in, read the same way
/// build.rs reads it (EPIC_CC_GIT_SHA first, then the tree's own git).
fn checkout_head() -> Option<String> {
    if let Ok(sha) = std::env::var("EPIC_CC_GIT_SHA") {
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

#[test]
fn version_flag_prints_identity_and_exits_zero() {
    assert!(!stamp().is_empty());
}

#[test]
fn short_version_flag_also_works() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg("-V")
        .output()
        .expect("run epic-cc -V");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("epic-cc "));
}

#[test]
fn a_set_version_reports_it_verbatim() {
    // EPIC_CC_VERSION is how the release stage names a bundle; when it is
    // set the stamp must be exactly that, with no sha appended, or a
    // release would report an identity no tag produced.
    match std::env::var("EPIC_CC_VERSION") {
        Ok(version) if !version.is_empty() => assert_eq!(stamp(), version),
        // Unset in a normal dev/CI run: the property is covered by the
        // sha test below instead.
        _ => {}
    }
}

#[test]
fn a_git_build_reports_the_checkout_sha() {
    if std::env::var("EPIC_CC_VERSION")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        // A release build: no sha is expected, the version is the identity.
        return;
    }
    let Some(head) = checkout_head() else {
        // No git checkout (a source tarball): the stamp is the bare crate
        // version, which is the honest answer and must still be non-empty.
        return;
    };
    let stamp = stamp();
    assert!(
        stamp.ends_with(&format!("+{head}")),
        "expected the checkout commit {head} in the stamp, got {stamp:?}"
    );
}
