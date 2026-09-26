//! The shipped `include/` directory (epic-cc#690, docs/46 D-6).
//!
//! An IDE cannot resolve headers materialized into a per-run temp dir, so
//! the release bundle ships `include/` next to the binary and the driver
//! resolves it exe-relative, the same chain shape as clang discovery. The
//! temp materialization stays as the fallback for dev/CI runs whose
//! executable has no bundle beside it. One table drives the dump, the
//! fallback and the presence check, so the three cannot drift apart.

use std::path::{Path, PathBuf};

use super::{
    epic_cc_h, malloc_h, math_h, stdarg_h, stdbool_h, stddef_h, stdint_h, stdio_h, stdlib_h,
    string_h, xc_h,
};

/// Every shipped header: filename plus its compiled-in text.
const HEADERS: &[(&str, &str)] = &[
    ("epic-cc.h", epic_cc_h::EPIC_CC_H),
    ("stdint.h", stdint_h::STDINT_H),
    ("stdbool.h", stdbool_h::STDBOOL_H),
    ("stddef.h", stddef_h::STDDEF_H),
    ("string.h", string_h::STRING_H),
    ("stdlib.h", stdlib_h::STDLIB_H),
    ("malloc.h", malloc_h::MALLOC_H),
    ("xc.h", xc_h::XC_H),
    ("stdarg.h", stdarg_h::STDARG_H),
    ("stdio.h", stdio_h::STDIO_H),
    ("math.h", math_h::MATH_H),
];

/// Write every header into `dir`, creating it when missing. Used by the
/// compile fallback and by `--dump-include-dir` when assembling the
/// release bundle, so the bundle carries exactly what the driver burns.
pub fn materialize(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in HEADERS {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(())
}

/// The shipped `<exe_dir>/include` when it holds headers, else `None`.
/// The anchor is `epic-cc.h`: the bundle dump writes all eleven files or
/// fails, so one anchor proves the set.
pub fn bundled(exe_dir: &Path) -> Option<PathBuf> {
    let dir = exe_dir.join("include");
    if dir.join("epic-cc.h").is_file() {
        Some(dir)
    } else {
        None
    }
}

/// Resolve the effective header dir: the bundle beside the executable
/// when present, else the materialized fallback under `fallback_parent`.
/// `fallback_parent` is the compile temp dir on the build path and a
/// per-run temp dir for the `--print-include-dir` query.
pub fn resolve(exe_dir: &Path, fallback_parent: &Path) -> std::io::Result<PathBuf> {
    if let Some(dir) = bundled(exe_dir) {
        return Ok(dir);
    }
    let dir = fallback_parent.join("include");
    materialize(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bundle(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("epic-cc-include-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bundled_finds_a_shipped_include_dir() {
        let bundle = fake_bundle("shipped");
        materialize(&bundle.join("include")).unwrap();
        assert_eq!(bundled(&bundle), Some(bundle.join("include")));
        std::fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn bundled_ignores_an_empty_include_dir() {
        let bundle = fake_bundle("empty");
        std::fs::create_dir_all(bundle.join("include")).unwrap();
        assert_eq!(bundled(&bundle), None);
        std::fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn bundled_ignores_a_missing_include_dir() {
        let bundle = fake_bundle("missing");
        assert_eq!(bundled(&bundle), None);
        std::fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn resolve_prefers_the_bundle_over_the_fallback() {
        let bundle = fake_bundle("prefer");
        materialize(&bundle.join("include")).unwrap();
        let fallback = fake_bundle("prefer-fallback");
        assert_eq!(resolve(&bundle, &fallback).unwrap(), bundle.join("include"));
        assert!(!fallback.join("include").exists());
        std::fs::remove_dir_all(&bundle).unwrap();
        std::fs::remove_dir_all(&fallback).unwrap();
    }

    #[test]
    fn resolve_materializes_the_fallback_when_unbundled() {
        let exe = fake_bundle("unbundled");
        let fallback = fake_bundle("unbundled-fallback");
        let dir = resolve(&exe, &fallback).unwrap();
        assert_eq!(dir, fallback.join("include"));
        assert!(dir.join("epic-cc.h").is_file());
        assert!(dir.join("stdint.h").is_file());
        assert!(dir.join("xc.h").is_file());
        std::fs::remove_dir_all(&exe).unwrap();
        std::fs::remove_dir_all(&fallback).unwrap();
    }
}
