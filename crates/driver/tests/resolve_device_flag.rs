//! `--resolve-device` exposes `device::resolve()` externally (epic-cc#428,
//! found spiking epic-hal#162): a caller that cannot link the `device`
//! crate, like a Python tool, still needs the same tolerant canonicalization
//! `--target` already applies internally.

fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(args)
        .output()
        .expect("run epic-cc")
}

#[test]
fn resolves_every_tolerant_spelling_to_the_canonical_name() {
    for spelling in ["p16f877a", "16F877A", "PIC16F877A", "16f877a"] {
        let out = run(&["--resolve-device", spelling]);
        assert!(
            out.status.success(),
            "spelling {spelling:?}, exit code: {:?}, stderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "p16f877a",
            "spelling {spelling:?}"
        );
        assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
    }
}

#[test]
fn unknown_device_fails_loudly_with_the_available_list() {
    let out = run(&["--resolve-device", "bogus"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown device bogus"), "stderr: {err}");
    assert!(err.contains("available:"), "stderr: {err}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
}

#[test]
fn missing_value_fails_with_a_clear_message() {
    let out = run(&["--resolve-device"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--resolve-device needs a value"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn needs_no_input_files() {
    // The ordinary compile path requires at least one input file; this
    // standalone query mode must not, since it never compiles anything.
    let out = run(&["--resolve-device", "p16f877a"]);
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("no input files"));
}
