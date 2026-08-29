// epic-cc#175 acceptance: front-end-stage panics name the C source
// location that caused them, resolved from clang's `-gline-tables-only`
// metadata. The three diagnostic classes are the ones the issue observed
// in practice: unsupported type, undefined symbol, recursion.

use std::process::Command;

fn run_driver(name: &str, src: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("epic-cc-175-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join(format!("{name}.c"));
    std::fs::write(&c_path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            c_path.to_str().unwrap(),
            "-o",
            dir.join("out.hex").to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.success(), stderr)
}

#[test]
fn unsupported_type_panic_names_the_definition_site() {
    let (ok, stderr) = run_driver(
        "spike_double",
        "long double get(void) { return 1.5L; }\nint main(void) { return (int)get(); }\n",
    );
    assert!(!ok, "long double must be rejected, not compiled");
    assert!(
        stderr.contains("spike_double.c:1:1: SPIKE: unsupported type \"double\""),
        "panic must carry the C location:\n{stderr}"
    );
}

#[test]
fn undefined_symbol_names_the_call_site() {
    let (ok, stderr) = run_driver(
        "spike_frob",
        "void frobnicate(void);\nint main(void) { frobnicate(); return 0; }\n",
    );
    assert!(!ok, "an undefined symbol must be rejected, not compiled");
    assert!(
        stderr.contains("undefined symbols: frobnicate (called at ")
            && stderr.contains("spike_frob.c:2:18"),
        "panic must carry the C location:\n{stderr}"
    );
}

#[test]
fn recursion_panic_names_the_recursive_call() {
    let (ok, stderr) = run_driver(
        "spike_recursion",
        "volatile int sink;\nint is_even(int n);\n\
         int is_odd(int n) { if (n == 0) return 0; return is_even(n - 1) + sink; }\n\
         int is_even(int n) { if (n == 0) return 1; return is_odd(n - 1) + sink; }\n\
         int main(void) { return is_odd(3); }\n",
    );
    assert!(!ok, "recursion must be rejected, not compiled");
    assert!(
        stderr.contains("spike_recursion.c:4:51") && stderr.contains("recursion detected"),
        "panic must carry the C location:\n{stderr}"
    );
}
