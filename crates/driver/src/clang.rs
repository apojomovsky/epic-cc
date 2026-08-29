//! Thin wrapper around the pinned clang invocation.
//!
//! The PIC front end is always `clang -target msp430 -O1 -S -emit-llvm
//! -ffreestanding -nostdinc -gline-tables-only -resource-dir <resdir>`,
//! that list is the input-format contract (docs/01 §-target, AGENTS.md).
//! Duplicating it across `main.rs` and two dozen e2e tests is how a flag
//! drift goes unnoticed. This module is the single source for those flags.
//!
//! `-gline-tables-only` rides in the contract: it adds line-table debug
//! metadata that `irparse` resolves into the `file.c:line:col` of
//! backend-stage panic messages, and nothing else the pipeline reads.
//!
//! Two layers:
//! - `base_cmd` returns a `Command` pre-loaded with the fixed flags,
//!   caller adds `-I`/`-D`/`-o`/input.
//! - `Options` + `try_compile_to_stdout` / `try_compile_to_file` (and their
//!   panicking `compile_*` siblings) handles the common whole-invocation
//!   plus success check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Fixed flags that define the PIC front-end contract. `-resource-dir`'s
/// value follows this array and is supplied by the caller.
const BASE_ARGS: &[&str] = &[
    "-target",
    "msp430",
    "-O1",
    "-S",
    "-emit-llvm",
    "-ffreestanding",
    "-nostdinc",
    "-gline-tables-only",
    "-resource-dir",
];

/// Extra per-invocation options. Mirrors the `-I`/`-D` surface `main.rs`
/// forwards to clang.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// `-I` include paths (`cli.includes`).
    pub includes: Vec<String>,
    /// `-D` defines (`cli.defines`).
    pub defines: Vec<String>,
    /// The `tmp/include` dir the driver materialises (`epic-cc.h`, `stdint.h`,
    /// …). Added as `-I <header_dir>` when `Some`.
    pub header_dir: Option<PathBuf>,
    /// `EPIC_FOSC_HZ` value. Added as `-D EPIC_FOSC_HZ=<hz>` when `Some`.
    pub fosc_hz: Option<u64>,
    /// `-fpack-struct`: the XC8 PIC18 record layout gives every struct
    /// member byte alignment (epic-cc#166). irparse reads the packedness
    /// back from the `<{ ... }>` types clang prints, so nothing past the
    /// front end needs the flag.
    pub packed_structs: bool,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&self, cmd: &mut Command) {
        apply_options(cmd, self);
    }
}

/// Build `Command::new(clang)` with the fixed PIC flags and `-resource-dir
/// <resdir>`. No `-o`, no input, no `-I`/`-D`, caller adds those.
pub fn base_cmd(clang: &Path, resdir: &Path) -> Command {
    let mut cmd = Command::new(clang);
    cmd.args(BASE_ARGS);
    cmd.arg(resdir);
    cmd
}

pub fn apply_options(cmd: &mut Command, opts: &Options) {
    for inc in &opts.includes {
        cmd.args(["-I", inc]);
    }
    for def in &opts.defines {
        cmd.args(["-D", def]);
    }
    if let Some(dir) = &opts.header_dir {
        cmd.args(["-I", dir.to_str().unwrap()]);
    }
    if let Some(hz) = opts.fosc_hz {
        cmd.args(["-D", &format!("EPIC_FOSC_HZ={hz}")]);
    }
    if opts.packed_structs {
        cmd.arg("-fpack-struct");
    }
}

/// Read the dev-container env pair (`PIC8_CLANG_UNWRAPPED` +
/// `PIC8_CLANG_RESOURCE_DIR`). Panics with the same `expect` messages the
/// e2e tests historically used, so existing failure output stays familiar.
pub fn pic_clang_from_env() -> (PathBuf, PathBuf) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    (PathBuf::from(clang), PathBuf::from(resdir))
}

/// Run clang for `src`, capture stdout LLVM IR. `Ok(ll)` on success,
/// `Err(stderr)` on clang failure or UTF-8 error.
pub fn try_compile_to_stdout(
    clang: &Path,
    resdir: &Path,
    src: &Path,
    opts: &Options,
) -> Result<String, String> {
    let mut cmd = base_cmd(clang, resdir);
    apply_options(&mut cmd, opts);
    cmd.args(["-o", "-"]);
    cmd.arg(src);
    let out = cmd.output().map_err(|e| format!("run clang: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    String::from_utf8(out.stdout).map_err(|e| format!("clang stdout: {e}"))
}

/// Like `try_compile_to_stdout` but panics on failure, the contract the
/// e2e tests rely on (`expect("run clang")` + `assert!(success)`).
pub fn compile_to_stdout(clang: &Path, resdir: &Path, src: &Path, opts: &Options) -> String {
    let mut cmd = base_cmd(clang, resdir);
    apply_options(&mut cmd, opts);
    cmd.args(["-o", "-"]);
    cmd.arg(src);
    let out = cmd.output().expect("run clang");
    assert!(
        out.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("clang stdout utf8")
}

/// Run clang for `src` into `out_file`. `Ok(())` on success.
pub fn try_compile_to_file(
    clang: &Path,
    resdir: &Path,
    src: &Path,
    out_file: &Path,
    opts: &Options,
) -> Result<(), String> {
    let mut cmd = base_cmd(clang, resdir);
    apply_options(&mut cmd, opts);
    cmd.arg("-o");
    cmd.arg(out_file);
    cmd.arg(src);
    let out = cmd.output().map_err(|e| format!("run clang: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(())
}

/// Panicking variant of `try_compile_to_file`.
pub fn compile_to_file(clang: &Path, resdir: &Path, src: &Path, out_file: &Path, opts: &Options) {
    let mut cmd = base_cmd(clang, resdir);
    apply_options(&mut cmd, opts);
    cmd.arg("-o");
    cmd.arg(out_file);
    cmd.arg(src);
    let out = cmd.output().expect("run clang");
    assert!(
        out.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_cmd_contains_fixed_flags() {
        let cmd = base_cmd(
            Path::new("/opt/clang/bin/clang"),
            Path::new("/opt/clang/lib/clang/20"),
        );
        let dbg = format!("{cmd:?}");
        for flag in [
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-gline-tables-only",
            "-resource-dir",
        ] {
            assert!(dbg.contains(flag), "missing {flag} in {dbg}");
        }
        assert!(dbg.contains("/opt/clang/lib/clang/20"));
    }

    #[test]
    fn options_adds_includes_and_defines() {
        let mut cmd = base_cmd(Path::new("clang"), Path::new("/res"));
        let opts = Options {
            includes: vec!["/inc/a".to_string()],
            defines: vec!["FOO=1".to_string()],
            header_dir: Some(PathBuf::from("/tmp/hdr")),
            fosc_hz: Some(20_000_000),
            packed_structs: false,
        };
        apply_options(&mut cmd, &opts);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("/inc/a"));
        assert!(dbg.contains("FOO=1"));
        assert!(dbg.contains("/tmp/hdr"));
        assert!(dbg.contains("EPIC_FOSC_HZ=20000000"));
        assert!(!dbg.contains("-fpack-struct"), "off by default");
    }

    #[test]
    fn options_add_fpack_struct_only_when_packed() {
        // PIC18 rides the XC8 record layout (every member byte-aligned,
        // epic-cc#166); the flag must appear only when the caller opts in.
        let mut cmd = base_cmd(Path::new("clang"), Path::new("/res"));
        apply_options(
            &mut cmd,
            &Options {
                packed_structs: true,
                ..Default::default()
            },
        );
        assert!(format!("{cmd:?}").contains("-fpack-struct"));
        let mut cmd = base_cmd(Path::new("clang"), Path::new("/res"));
        apply_options(&mut cmd, &Options::default());
        assert!(!format!("{cmd:?}").contains("-fpack-struct"));
    }
}
