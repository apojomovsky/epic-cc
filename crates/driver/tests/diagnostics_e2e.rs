//! Internal-error presentation vs user errors (epic-cc#692).
//!
//! Bugs print `epic-cc: internal compiler error:` with the version and the
//! issue URL; deliberate input errors print as errors, with `file:line:col`
//! where the driver knows the site.

use std::process::Command;

fn driver() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout).to_string()
}

fn tmp_c(name: &str, body: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "epic-diag-{}-{}-{name}",
        std::process::id(),
        name.len()
    ));
    std::fs::write(&path, body).expect("write temp C file");
    path.display().to_string()
}

#[test]
fn unknown_device_reports_an_error_not_an_ice() {
    let out = driver()
        .args([
            "tests/fixtures/config_probe.c",
            "-o",
            "/tmp/epic-diag-unknown.hex",
            "--device",
            "bogus-part",
        ])
        .output()
        .expect("run driver");
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let err = stderr_of(&out);
    assert!(
        err.contains("epic-cc: error: unknown device bogus-part"),
        "{err}"
    );
    assert!(!err.contains("internal compiler error"), "{err}");
}

#[test]
fn duplicate_epic_config_points_at_both_sites() {
    let a = tmp_c("dup-a.c", "EPIC_CONFIG(\"osc=xt\");\nvoid main(void) {}\n");
    let b = tmp_c(
        "dup-b.c",
        "\n\nEPIC_CONFIG(\"osc=hs\");\nvoid helper(void) {}\n",
    );
    let out = driver()
        .args([
            a.as_str(),
            b.as_str(),
            "-o",
            "/tmp/epic-diag-dup.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("error: more than one EPIC_CONFIG"), "{err}");
    assert!(err.contains("dup-a.c:1:1"), "{err}");
    assert!(err.contains("dup-b.c:3:1"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
}

#[test]
fn unknown_config_value_lists_the_valid_options() {
    let src = tmp_c(
        "badval.c",
        "EPIC_CONFIG(\"osc=turbo\");\nvoid main(void) {}\n",
    );
    let out = driver()
        .args([
            src.as_str(),
            "-o",
            "/tmp/epic-diag-badval.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&src);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains(":1:1: error:"), "{err}");
    assert!(
        err.contains("unknown value 'turbo' for field 'osc'"),
        "{err}"
    );
    assert!(err.contains("expected one of"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
}

#[test]
fn unsupported_asm_stays_an_error_not_an_ice() {
    let out = driver()
        .args([
            "tests/fixtures/asm_reg.c",
            "-o",
            "/tmp/epic-diag-asmreg.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(
        err.contains("register constraints are not supported"),
        "{err}"
    );
    assert!(!err.contains("internal compiler error"), "{err}");
}

#[test]
fn recursion_reports_a_located_error_not_an_ice() {
    let src = tmp_c(
        "rec.c",
        "volatile int sink;\nint is_even(int n);\n\
         int is_odd(int n) { if (n == 0) return 0; return is_even(n - 1) + sink; }\n\
         int is_even(int n) { if (n == 0) return 1; return is_odd(n - 1) + sink; }\n\
         int main(void) { return is_odd(3); }\n",
    );
    let out = driver()
        .args([
            src.as_str(),
            "-o",
            "/tmp/epic-diag-rec.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&src);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains(": error:"), "{err}");
    assert!(err.contains("recursion detected"), "{err}");
    assert!(err.contains("rec.c:4:51"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
}
