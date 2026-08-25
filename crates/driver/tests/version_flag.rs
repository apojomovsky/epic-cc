//! `--version` prints the compiler identity a downstream job can pin and
//! report. The stamp comes from the driver build.rs: EPIC_CC_VERSION when set
//! (the docker release stage's build ARG), the crate version otherwise.

#[test]
fn version_flag_prints_identity_and_exits_zero() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg("--version")
        .output()
        .expect("run epic-cc --version");
    assert!(out.status.success(), "exit code: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().expect("--version prints a line");
    assert!(
        line.starts_with("epic-cc "),
        "expected 'epic-cc <stamp>', got: {line:?}"
    );
    let stamp = line.trim_start_matches("epic-cc ");
    assert!(!stamp.is_empty(), "stamp must not be empty");
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn short_version_flag_also_works() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg("-V")
        .output()
        .expect("run epic-cc -V");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("epic-cc "));
}
