//! Function labels whose names start with a directive keyword (epic-cc#1036)
//! must not stop or skip the label scans in the isel backends. `endpoint:` and
//! `listen:` are ordinary function labels; only the bare `end` directive and
//! whole-token `list`/`radix` directives are structural.

use std::process::Command;

#[test]
fn functions_named_with_directive_prefixes_compile_without_ice() {
    let pid = std::process::id();
    let src = std::env::temp_dir().join(format!("epic-label-prefix-{pid}.c"));
    let hex = std::env::temp_dir().join(format!("epic-label-prefix-{pid}.hex"));
    std::fs::write(
        &src,
        "volatile unsigned char p;\n\
         void endpoint(void) { p = 1; }\n\
         void listen(void) { p = 2; }\n\
         void main(void) { endpoint(); listen(); }\n",
    )
    .expect("write temp C file");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            src.display().to_string().as_str(),
            "-o",
            hex.display().to_string().as_str(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&hex);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
}
