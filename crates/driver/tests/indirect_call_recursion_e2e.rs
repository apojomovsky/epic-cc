// epic-cc#73 acceptance: a cycle through a function pointer is rejected by
// the callgraph depth/recursion check. `f` stores a callback that reaches
// back to `f` through an indirect call, so the conservative whole-program
// graph has a cycle and the driver must fail loudly rather than miscompile.

use std::process::Command;

#[test]
fn recursion_through_function_pointer_is_rejected() {
    let src = r#"
typedef void (*cb_t)(void);
volatile cb_t g_cb;
void f(void) { g_cb(); }
int main(void) {
    g_cb = f;
    f();
    return 0;
}
"#;
    let dir = std::env::temp_dir().join("epic-cc-73-recursion");
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join("recursion.c");
    std::fs::write(&c_path, src).unwrap();
    let hex_path = dir.join("recursion.hex");

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            c_path.to_str().unwrap(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        !out.status.success(),
        "recursion through a function pointer must be rejected, not compiled"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("recursion") || stderr.contains("cycle"),
        "rejection must name the recursion/cycle:\n{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
