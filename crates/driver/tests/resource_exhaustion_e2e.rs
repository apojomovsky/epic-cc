//! Lane F of docs/36 (approved design, #290): resource-exhaustion boundary
//! tests (the graceful-failure contract). A program that uses only
//! supported constructs but genuinely does not fit the target's RAM,
//! hardware call-stack depth, or flash must fail loudly with a specific,
//! actionable diagnostic and a non-zero exit code, not silently miscompile
//! or corrupt output.
//!
//! Each axis is tested at the exact-fit / one-past-fit boundary on the
//! p16f877a (4 GPR banks = 384 bytes, 8-deep hardware stack, 8192 words
//! flash). The RAM and flash fixtures are generated in-test (hundreds to
//! thousands of globals/functions), so they stay practical to keep in the
//! repo.

use std::process::Command;

/// Run the driver over `src` and return (exit_code, stderr).
fn run_driver(src: &str, device: &str, tag: &str) -> (i32, String) {
    let dir = std::env::temp_dir().join(format!("epic-cc-290-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join("prog.c");
    std::fs::write(&c_path, src).unwrap();
    let hex_path = dir.join("prog.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            c_path.to_str().unwrap(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            device,
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(&dir);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A chain of `n` noinline functions, each calling the previous, so the
/// call graph is a depth-`n` chain. `main` calls the deepest.
fn depth_chain(n: usize) -> String {
    let mut s = String::from("volatile unsigned char g;\n");
    s.push_str("__attribute__((noinline)) void f0(void) { g = 0; }\n");
    for i in 1..n {
        s.push_str(&format!(
            "__attribute__((noinline)) void f{i}(void) {{ f{}(); g = {i}; }}\n",
            i - 1
        ));
    }
    s.push_str(&format!("void main(void) {{ f{}(); }}\n", n - 1));
    s
}

/// `n` volatile 1-byte globals, so the allocator must place `n` bytes of
/// GPR. `main` touches the first and last to keep them live.
fn ram_globals(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!("volatile unsigned char g{i};\n"));
    }
    s.push_str(&format!("void main(void) {{ g0 = 1; g{} = 2; }}\n", n - 1));
    s
}

/// `n` noinline functions, each a few words, so the program grows toward
/// the device's flash. `main` calls the first ten to keep them live.
fn flash_functions(n: usize) -> String {
    let mut s = String::from("volatile unsigned char g;\n");
    for i in 0..n {
        s.push_str(&format!(
            "__attribute__((noinline)) void f{i}(void) {{ g = {i}; }}\n"
        ));
    }
    s.push_str("void main(void) { f0(); f1(); f2(); f3(); f4(); f5(); f6(); f7(); f8(); f9(); }\n");
    s
}

// --- Call-stack depth (p16f877a: 8-deep hardware stack) ---

#[test]
fn call_stack_exact_fit_compiles() {
    // 7 functions = depth 8 (7 calls + main), the exact fit.
    let (code, stderr) = run_driver(&depth_chain(7), "p16f877a", "depth7");
    assert_eq!(code, 0, "exact-fit depth must compile:\n{stderr}");
}

#[test]
fn call_stack_one_past_fails_with_diagnostic() {
    // 8 functions = depth 9, one past the 8-deep stack.
    let (code, stderr) = run_driver(&depth_chain(8), "p16f877a", "depth8");
    assert_ne!(code, 0, "one-past depth must be rejected, not compiled");
    assert!(
        stderr.contains("exceeds hardware stack"),
        "rejection must name the stack-depth limit:\n{stderr}"
    );
}

// --- RAM (p16f877a: 4 GPR banks = 384 bytes) ---

#[test]
fn ram_exact_fit_compiles() {
    // 350 globals fit (measured boundary: 350 ok, 355 fails).
    let (code, stderr) = run_driver(&ram_globals(350), "p16f877a", "ram350");
    assert_eq!(code, 0, "exact-fit RAM must compile:\n{stderr}");
}

#[test]
fn ram_one_past_fails_with_diagnostic() {
    // 355 globals, one past the RAM boundary.
    let (code, stderr) = run_driver(&ram_globals(355), "p16f877a", "ram355");
    assert_ne!(code, 0, "one-past RAM must be rejected, not compiled");
    assert!(
        stderr.contains("no arrangement") || stderr.contains("GPR demand"),
        "rejection must name the RAM/alloc limit:\n{stderr}"
    );
}

// --- Flash (p16f877a: 8192 words) ---

#[test]
fn flash_exact_fit_compiles() {
    // 8100 functions fit (measured boundary: 8100 ok, 8200 fails).
    let (code, stderr) = run_driver(&flash_functions(8100), "p16f877a", "flash8100");
    assert_eq!(code, 0, "exact-fit flash must compile:\n{stderr}");
}

#[test]
fn flash_one_past_fails_with_diagnostic() {
    // 8200 functions, one past the flash boundary.
    let (code, stderr) = run_driver(&flash_functions(8200), "p16f877a", "flash8200");
    assert_ne!(code, 0, "one-past flash must be rejected, not compiled");
    assert!(
        stderr.contains("flash") || stderr.contains("beyond page"),
        "rejection must name the flash/page limit:\n{stderr}"
    );
}
