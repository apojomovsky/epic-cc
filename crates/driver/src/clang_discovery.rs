//! clang front-end discovery for the epic-cc driver.
//!
//! The driver needs two things from clang: the binary and the resource dir
//! (builtin headers). Resolution order:
//!
//! 1. `PIC8_CLANG_UNWRAPPED` + `PIC8_CLANG_RESOURCE_DIR` env vars (dev/CI
//!    path — the docker images export these).
//! 2. Bundled: `<exe_dir>/clang/bin/clang` with the first subdirectory of
//!    `<exe_dir>/clang/lib/clang/` as the resource dir (release bundles).
//! 3. A clean error naming both options — never a panic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolve `(clang_binary, resource_dir)`.
pub fn resolve_clang(
    env: &HashMap<String, String>,
    exe_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    // 1. Env vars, both-or-neither.
    let env_clang = env.get("PIC8_CLANG_UNWRAPPED");
    let env_resdir = env.get("PIC8_CLANG_RESOURCE_DIR");
    if let (Some(clang), Some(resdir)) = (env_clang, env_resdir) {
        return Ok((PathBuf::from(clang), PathBuf::from(resdir)));
    }

    // 2. Bundled clang next to the executable.
    let bundled_clang = ["clang", "clang.exe"]
        .iter()
        .map(|name| exe_dir.join("clang").join("bin").join(name))
        .find(|p| p.is_file());
    if let Some(clang) = bundled_clang {
        let res_root = exe_dir.join("clang").join("lib").join("clang");
        if let Ok(entries) = std::fs::read_dir(&res_root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    return Ok((clang, entry.path()));
                }
            }
        }
        return Err(format!(
            "bundled clang found at {} but no resource dir under {}",
            clang.display(),
            res_root.display()
        ));
    }

    // 3. Clean diagnostic.
    Err("no clang front end found: set PIC8_CLANG_UNWRAPPED and \
         PIC8_CLANG_RESOURCE_DIR, or ship the clang/ directory next to the \
         executable"
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn fake_bundle(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("epic-cc-discovery-{tag}"));
        let bin = dir.join("clang").join("bin");
        let res = dir.join("clang").join("lib").join("clang").join("20");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(bin.join("clang"), "#!/bin/sh\n").unwrap();
        dir
    }

    #[test]
    fn env_vars_win() {
        let env = env_of(&[
            ("PIC8_CLANG_UNWRAPPED", "/usr/bin/clang"),
            ("PIC8_CLANG_RESOURCE_DIR", "/usr/lib/clang/20"),
        ]);
        let (clang, resdir) = resolve_clang(&env, Path::new("/nonexistent")).unwrap();
        assert_eq!(clang, PathBuf::from("/usr/bin/clang"));
        assert_eq!(resdir, PathBuf::from("/usr/lib/clang/20"));
    }

    #[test]
    fn bundled_fallback() {
        let dir = fake_bundle("bundled");
        let (clang, resdir) = resolve_clang(&HashMap::new(), &dir).unwrap();
        assert_eq!(clang, dir.join("clang").join("bin").join("clang"));
        assert_eq!(
            resdir,
            dir.join("clang").join("lib").join("clang").join("20")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_windows_exe_name() {
        let dir = fake_bundle("windows");
        std::fs::rename(
            dir.join("clang").join("bin").join("clang"),
            dir.join("clang").join("bin").join("clang.exe"),
        )
        .unwrap();
        let (clang, _) = resolve_clang(&HashMap::new(), &dir).unwrap();
        assert_eq!(clang, dir.join("clang").join("bin").join("clang.exe"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn neither_errors() {
        let err = resolve_clang(&HashMap::new(), Path::new("/nonexistent")).unwrap_err();
        assert!(err.contains("PIC8_CLANG_UNWRAPPED"), "err: {err}");
    }
}

/// Find `llvm-link` beside the clang that `resolve_clang` returned. Both the
/// dev image and the release bundle ship them in the same directory, so the
/// clang path is the only input needed.
pub fn resolve_llvm_link(clang: &Path) -> Result<PathBuf, String> {
    let dir = clang
        .parent()
        .ok_or_else(|| format!("clang path has no parent directory: {}", clang.display()))?;
    for name in ["llvm-link", "llvm-link.exe"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!(
        "llvm-link not found next to clang in {}. It ships with the toolchain bundle; \
         set PIC8_CLANG_UNWRAPPED to a clang whose directory also contains llvm-link.",
        dir.display()
    ))
}

/// Find `opt` beside the clang that `resolve_clang` returned, the same way
/// `resolve_llvm_link` finds `llvm-link`.
pub fn resolve_opt(clang: &Path) -> Result<PathBuf, String> {
    let dir = clang
        .parent()
        .ok_or_else(|| format!("clang path has no parent directory: {}", clang.display()))?;
    for name in ["opt", "opt.exe"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!(
        "opt not found next to clang in {}. It ships with the toolchain bundle; \
         set PIC8_CLANG_UNWRAPPED to a clang whose directory also contains opt.",
        dir.display()
    ))
}
