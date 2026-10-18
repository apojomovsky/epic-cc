use driver::cli::{parse_args, Emit};

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

#[test]
fn parses_a_minimal_invocation() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.inputs, vec!["a.c"]);
    assert_eq!(c.output, "a.hex");
    assert_eq!(c.device, "p16f877a");
    assert!(matches!(c.emit, Emit::Hex));
}

#[test]
fn const_pool_defaults_on_with_opt_out() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert!(c.const_pool);
    let c = parse_args(&args(&["a.c", "--device", "p16f877a", "--no-const-pool"])).unwrap();
    assert!(!c.const_pool);
    let c = parse_args(&args(&["a.c", "--device", "p16f877a", "--const-pool"])).unwrap();
    assert!(c.const_pool);
}

#[test]
fn parses_an_f_cpu_board_clock() {
    let c = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--f-cpu", "4000000",
    ]))
    .unwrap();
    assert_eq!(c.f_cpu, Some(4_000_000));
    let minimal = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(minimal.f_cpu, None);
}

#[test]
fn collects_multiple_inputs_includes_and_defines() {
    let c = parse_args(&args(&[
        "a.c", "b.c", "-I", "inc", "-I", "inc2", "-D", "F=1", "-D", "G", "-o", "out.hex",
        "--device", "p18f4550",
    ]))
    .unwrap();
    assert_eq!(c.inputs, vec!["a.c", "b.c"]);
    assert_eq!(c.includes, vec!["inc", "inc2"]);
    assert_eq!(c.defines, vec!["F=1", "G"]);
    assert_eq!(c.output, "out.hex");
    assert_eq!(c.device, "p18f4550");
}

#[test]
fn accepts_attached_short_flag_forms() {
    let c = parse_args(&args(&[
        "a.c",
        "-Iinc",
        "-DF=1",
        "-oout.hex",
        "--device",
        "p16f877a",
    ]))
    .unwrap();
    assert_eq!(c.includes, vec!["inc"]);
    assert_eq!(c.defines, vec!["F=1"]);
    assert_eq!(c.output, "out.hex");
}

#[test]
fn parses_emit_stages() {
    for (s, want) in [
        ("ll", Emit::Ll),
        ("ir", Emit::Ir),
        ("asm", Emit::Asm),
        ("hex", Emit::Hex),
    ] {
        let c = parse_args(&args(&["a.c", "--device", "p16f877a", "--emit", s])).unwrap();
        assert_eq!(c.emit, want);
    }
}

#[test]
fn rejects_a_missing_device() {
    let e = parse_args(&args(&["a.c"])).unwrap_err();
    assert!(e.contains("--target"), "{e}");
}

#[test]
fn rejects_no_inputs() {
    let e = parse_args(&args(&["--device", "p16f877a"])).unwrap_err();
    assert!(e.contains("no input files"), "{e}");
}

#[test]
fn rejects_an_unknown_flag() {
    let e = parse_args(&args(&["a.c", "--device", "p16f877a", "--wat"])).unwrap_err();
    assert!(e.contains("--wat"), "{e}");
}

#[test]
fn rejects_an_unknown_emit_stage() {
    let e = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--emit", "bytecode",
    ]))
    .unwrap_err();
    assert!(e.contains("bytecode"), "{e}");
}

#[test]
fn rejects_a_flag_missing_its_value() {
    let e = parse_args(&args(&["a.c", "--device"])).unwrap_err();
    assert!(e.contains("--device"), "{e}");
}

#[test]
fn parses_a_map_file() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a", "--map", "out.map"])).unwrap();
    assert_eq!(c.map.as_deref(), Some("out.map"));
}

#[test]
fn map_defaults_to_none() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.map, None);
}

#[test]
fn parses_a_report_file() {
    let c = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--report", "b.json",
    ]))
    .unwrap();
    assert_eq!(c.report.as_deref(), Some("b.json"));
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.report, None);
}

#[test]
fn rejects_a_bad_or_zero_f_cpu() {
    assert!(parse_args(&args(&["a.c", "--device", "p16f877a", "--f-cpu", "abc"])).is_err());
    assert!(parse_args(&args(&["a.c", "--device", "p16f877a", "--f-cpu", "0"])).is_err());
    assert!(parse_args(&args(&["a.c", "--device", "p16f877a", "--f-cpu"])).is_err());
    let c = parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--f-cpu",
        "4_000_000",
    ]))
    .unwrap();
    assert_eq!(c.f_cpu, Some(4_000_000));
}

#[test]
fn parses_all_four_opt_levels() {
    use driver::cli::OptLevel;
    for (flag, want) in [
        ("-O0", OptLevel::O0),
        ("-O1", OptLevel::O1),
        ("-O2", OptLevel::O2),
        ("-Os", OptLevel::Os),
    ] {
        let c = parse_args(&args(&["a.c", "--device", "p16f877a", flag])).unwrap();
        assert_eq!(c.opt_level, want, "{flag}");
    }
}

#[test]
fn opt_level_defaults_to_os_and_last_flag_wins() {
    use driver::cli::OptLevel;
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.opt_level, OptLevel::Os);
    let c = parse_args(&args(&["a.c", "--device", "p16f877a", "-O2", "-O0"])).unwrap();
    assert_eq!(c.opt_level, OptLevel::O0);
}

#[test]
fn rejects_an_unknown_opt_level() {
    let e = parse_args(&args(&["a.c", "--device", "p16f877a", "-O3"])).unwrap_err();
    assert!(e.contains("-O3"), "{e}");
    let e = parse_args(&args(&["a.c", "--device", "p16f877a", "-O"])).unwrap_err();
    assert!(e.contains("-O"), "{e}");
}

#[test]
fn rejects_emit_asm_with_line_table() {
    let e = parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--emit",
        "asm",
        "--line-table",
        "t.txt",
    ]))
    .unwrap_err();
    assert!(e.contains("--line-table"), "{e}");
    assert!(e.contains("--emit asm"), "{e}");
}

#[test]
fn rejects_emit_ir_with_map() {
    let e = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--emit", "ir", "--map", "m.txt",
    ]))
    .unwrap_err();
    assert!(e.contains("--map"), "{e}");
    assert!(e.contains("--emit ir"), "{e}");
}

#[test]
fn rejects_emit_ll_with_report() {
    let e = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--emit", "ll", "--report", "r.json",
    ]))
    .unwrap_err();
    assert!(e.contains("--report"), "{e}");
    assert!(e.contains("--emit ll"), "{e}");
}

#[test]
fn rejects_emit_ll_with_var_table() {
    let e = parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--emit",
        "ll",
        "--var-table",
        "v.txt",
    ]))
    .unwrap_err();
    assert!(e.contains("--var-table"), "{e}");
    assert!(e.contains("--emit ll"), "{e}");
}

#[test]
fn allows_emit_asm_with_map() {
    let c = parse_args(&args(&[
        "a.c", "--device", "p16f877a", "--emit", "asm", "--map", "m.txt",
    ]))
    .unwrap();
    assert!(c.map.is_some());
}

#[test]
fn allows_emit_hex_with_every_artifact() {
    parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--emit",
        "hex",
        "--map",
        "m.txt",
        "--var-table",
        "v.txt",
        "--line-table",
        "t.txt",
        "--sidecar",
        "s.bin",
        "--report",
        "r.json",
    ]))
    .unwrap();
}

// a.c need not exist below: the argument check runs before any file is read.
fn rejected_invocation(emit: &str, flag: &str, path: &std::path::Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["a.c", "--device", "p16f877a", "--emit", emit, flag])
        .arg(path)
        .output()
        .expect("run driver")
}

fn tmp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("epic-cc-emit-reject-{}-{tag}", std::process::id()))
}

#[test]
fn emit_asm_with_line_table_exits_2_naming_both_flags() {
    let path = tmp_path("line-table.txt");
    let out = rejected_invocation("asm", "--line-table", &path);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--line-table"), "{stderr}");
    assert!(stderr.contains("--emit asm"), "{stderr}");
    assert!(!path.exists(), "rejected build must not write the artifact");
}

#[test]
fn emit_ir_with_map_exits_2_naming_both_flags() {
    let path = tmp_path("map.txt");
    let out = rejected_invocation("ir", "--map", &path);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--map"), "{stderr}");
    assert!(stderr.contains("--emit ir"), "{stderr}");
    assert!(!path.exists(), "rejected build must not write the artifact");
}

#[test]
fn emit_ll_with_report_exits_2_naming_both_flags() {
    let path = tmp_path("report.json");
    let out = rejected_invocation("ll", "--report", &path);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--report"), "{stderr}");
    assert!(stderr.contains("--emit ll"), "{stderr}");
    assert!(!path.exists(), "rejected build must not write the artifact");
}

#[test]
fn emit_hex_with_every_artifact_writes_each_file() {
    let tag = format!("epic-cc-emit-hex-{}", std::process::id());
    let dir = std::env::temp_dir();
    let (hex, map, var_table, line_table, sidecar, report) = (
        dir.join(format!("{tag}.hex")),
        dir.join(format!("{tag}.map")),
        dir.join(format!("{tag}.vars")),
        dir.join(format!("{tag}.lines")),
        dir.join(format!("{tag}.sidecar")),
        dir.join(format!("{tag}.json")),
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/add.c",
            "--device",
            "p16f877a",
            "--emit",
            "hex",
            "-o",
        ])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .arg("--var-table")
        .arg(&var_table)
        .arg("--line-table")
        .arg(&line_table)
        .arg("--sidecar")
        .arg(&sidecar)
        .arg("--report")
        .arg(&report)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for p in [&hex, &map, &var_table, &line_table, &sidecar, &report] {
        assert!(p.exists(), "missing {}", p.display());
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn parses_a_sidecar_file() {
    let c = parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--sidecar",
        "t.elf",
    ]))
    .unwrap();
    assert_eq!(c.sidecar.as_deref(), Some("t.elf"));
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.sidecar, None);
}

#[test]
fn usage_documents_every_long_flag() {
    use driver::cli::USAGE;
    for flag in [
        "--target",
        "--device",
        "--mcu",
        "--emit",
        "--save-temps",
        "--map",
        "--line-table",
        "--var-table",
        "--sidecar",
        "--report",
        "--f-cpu",
        "--no-outline",
        "--const-pool",
        "--no-const-pool",
        "--version",
        "--help",
        "--resolve-device",
        "--print-include-dir",
        "--dump-include-dir",
    ] {
        assert!(USAGE.contains(flag), "USAGE omits {flag}");
    }
}
