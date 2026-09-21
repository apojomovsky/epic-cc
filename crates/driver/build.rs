//! Stamp the built binary with the identity a downstream job can pin.
//!
//! EPIC_CC_VERSION (the docker `release` stage's build ARG) is authoritative
//! when set, so a release bundle reports its release version. Otherwise the
//! checkout's commit is appended as semver build metadata, which is what lets
//! a consumer detect a binary left behind by an older commit (epic-hal#240).
//! With no git checkout, or no git, the crate version stands alone rather
//! than failing the build.
//!
//! The `.git` rerun hints are what keep the stamp honest: cargo keys a
//! rerun on them, so a commit or branch switch rebuilds and the sha
//! follows HEAD instead of freezing at whatever the last source edit saw.

use std::process::Command;

/// The commit this checkout is on, or `None` when that cannot be known.
///
/// `EPIC_CC_GIT_SHA` wins: the Makefile resolves it on the host, where a
/// worktree's `.git` gitfile path is valid, because inside the container
/// that absolute path is outside the mount and `git` cannot follow it. The
/// in-tree call is the fallback for a plain host `cargo build`.
///
/// Any failure means "no identity", never a failed build: a release tarball
/// has no `.git` at all, and a sandbox may have no `git` on PATH.
fn head_sha() -> Option<String> {
    if let Ok(sha) = std::env::var("EPIC_CC_GIT_SHA") {
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn stamp() -> String {
    // Empty is treated as unset: an unset build ARG and an empty one both
    // mean "no release version", and an empty stamp would print bare.
    if let Ok(version) = std::env::var("EPIC_CC_VERSION") {
        if !version.is_empty() {
            return version;
        }
    }
    match head_sha() {
        Some(sha) => format!("{}+{sha}", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn main() {
    println!("cargo:rustc-env=EPIC_CC_STAMP={}", stamp());
    println!("cargo:rerun-if-env-changed=EPIC_CC_VERSION");
    println!("cargo:rerun-if-env-changed=EPIC_CC_GIT_SHA");
    // In a linked worktree `.git` is a file, not a directory, and these
    // paths resolve through it the same way.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
