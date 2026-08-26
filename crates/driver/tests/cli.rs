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
