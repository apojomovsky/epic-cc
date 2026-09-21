//! Stamp the built binary with the identity a downstream job can pin.
//!
//! EPIC_CC_VERSION (the docker `release` stage's build ARG) is authoritative
//! when set, so a release bundle reports its release version. Otherwise the
//! checkout's commit is appended as semver build metadata, which is what lets
//! a consumer detect a binary left behind by an older commit (epic-hal#240).
//! With no git checkout, or no git, the crate version stands alone rather
//! than failing the build.
//!
//! The rerun hints are what keep the stamp honest: cargo re-runs this script
//! when the git dir moves, so the sha follows HEAD instead of freezing at
//! whatever the last source edit saw.

use std::path::{Path, PathBuf};
use std::process::Command;

/// This crate's directory, and the repo root two levels above it.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_dir().join("..").join("..")
}

/// Run `git <args>` in `dir`, returning trimmed stdout on success.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The repo this crate's build belongs to, if git agrees it is the checkout.
///
/// git walks up from the crate dir, so a source tree with no `.git` of its
/// own but nested inside some other repo would otherwise stamp that repo's
/// commit: a dishonest identity, and one a consumer comparing against its
/// own epic-cc checkout would read as stale. Requiring the discovered
/// toplevel to be this repo's root rejects that case.
fn checkout() -> Option<PathBuf> {
    let root = repo_root();
    let found = git(&crate_dir(), &["rev-parse", "--show-toplevel"])?;
    let found = std::fs::canonicalize(found).ok()?;
    let expected = std::fs::canonicalize(&root).ok()?;
    (found == expected).then_some(root)
}

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
    let root = checkout()?;
    git(&root, &["rev-parse", "--short", "HEAD"])
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

/// Ask cargo to re-run this script when the commit changes.
///
/// The paths must be absolute, and must exist. A relative
/// `rerun-if-changed` resolves against this package's root, so `.git/HEAD`
/// would name `crates/driver/.git/HEAD`; cargo treats a missing path as
/// permanently dirty and rebuilds the driver on every build. Only paths git
/// actually reports are emitted, since a linked worktree has neither a
/// `packed-refs` of its own nor always a loose ref file.
fn emit_rerun_hints() {
    let Some(root) = checkout() else {
        return;
    };
    let Some(git_dir) = git(&root, &["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    // HEAD moves on checkout, branch switch and commit; the other two cover
    // where a commit lands, packed or loose.
    let mut paths = vec![format!("{git_dir}/HEAD")];
    for extra in ["packed-refs", "refs/heads"] {
        let candidate = format!("{git_dir}/{extra}");
        if Path::new(&candidate).exists() {
            paths.push(candidate);
        }
    }
    if let Some(ref_name) = git(&root, &["rev-parse", "--symbolic-full-name", "HEAD"]) {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", &ref_name]) {
            if Path::new(&path).exists() {
                paths.push(path);
            }
        }
    }
    for path in paths {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    println!("cargo:rustc-env=EPIC_CC_STAMP={}", stamp());
    println!("cargo:rerun-if-env-changed=EPIC_CC_VERSION");
    println!("cargo:rerun-if-env-changed=EPIC_CC_GIT_SHA");
    emit_rerun_hints();
}
