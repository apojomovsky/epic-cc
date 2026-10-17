//! PIC12 baseline debugger acceptance (epic-cc#1313): the phase-4 session
//! shape rerun on the baseline core, proving the word-address line table,
//! the reused typed variables, and the `PicBaselineTarget` adapter together.
//! A real gdb session against `serve_once` with `Core::PicBaseline` on a
//! `p12f509` build: `break file:line` from the sidecar `.debug_line`,
//! `continue` stops with source, `print` of a global and a local reads the
//! allocated address, `next` advances a line.
use std::process::Command;

use device::Core;
use epic_cc_gdbserver::load_program;

/// Compile `src` to HEX plus the debugger artifacts, all under `dir`.
fn compile(src: &str, dir: &std::path::Path) -> (String, String, String) {
    let hex = dir.join("t.hex").to_string_lossy().into_owned();
    let elf = dir.join("t.elf").to_string_lossy().into_owned();
    let lt = dir.join("t.lt").to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            src,
            "-o",
            &hex,
            "--device",
            "p12f509",
            "--sidecar",
            &elf,
            "--line-table",
            &lt,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (hex, elf, lt)
}

/// Run one gdb batch session against `serve_once(prog)` and return the
/// combined output.
fn gdb_session(elf: &str, prog: Vec<u16>, cmds: &[String]) -> String {
    let (addr, server) =
        epic_cc_gdbserver::serve_once(prog, Core::PicBaseline, Some(&device::PIC12F509))
            .expect("bind server");
    let mut args = vec![
        "-batch".to_string(),
        "-ex".to_string(),
        format!("file {elf}"),
        "-ex".to_string(),
        format!("target remote 127.0.0.1:{}", addr.port()),
    ];
    for c in cmds {
        args.push("-ex".to_string());
        args.push(c.clone());
    }
    let out = Command::new("gdb").args(&args).output().expect("run gdb");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    server.join().expect("server thread");
    text
}

/// The `(line, addr)` records of a `--line-table` file.
fn line_records(lt: &str) -> Vec<(u32, u16)> {
    std::fs::read_to_string(lt)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with(';'))
        .map(|l| {
            let mut it = l.split_whitespace();
            let loc = it.next().unwrap();
            let addr = it.next().unwrap();
            let line: u32 = loc.rsplit(':').nth(1).unwrap().parse().unwrap();
            let addr = u16::from_str_radix(&addr[2..], 16).unwrap();
            (line, addr)
        })
        .collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dbg1313-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pic12_session_break_print_step() {
    let dir = scratch("session");
    let src = "tests/fixtures/debug_session_pic12.c";
    let (hex, elf, lt) = compile(src, &dir);
    let prog = load_program(&hex, &elf, Core::PicBaseline).expect("load program");

    // The sidecar's .debug_line must agree with --line-table on every
    // mapped word address: ask gdb for each address in one session.
    let records = line_records(&lt);
    assert!(!records.is_empty(), "line table must have rows");
    let mut cmds: Vec<String> = records
        .iter()
        .map(|(_, addr)| format!("info line *{addr:#x}"))
        .collect();
    // Then the interactive session: stop at line 6 before anything ran,
    // seed the volatile input through the stub's memory write path, and
    // continue to line 8. The fixture's globals are volatile so each
    // store reaches its home address: stopped-at lines print fresh
    // values, never temps the backend forwarded past a plain store.
    // Lines 6-7 ran before the stop, so `g_cnt` is 41 and `loc` is 42.
    cmds.extend(
        [
            "break debug_session_pic12.c:6",
            "break debug_session_pic12.c:8",
            "continue",
            "set variable g_in = 41",
            "continue",
            "print g_cnt",
            "print loc",
            "print g_in",
            "next",
            "info line",
            "print g_cnt",
            "continue",
            "quit",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let text = gdb_session(&elf, prog, &cmds);

    // Every line-table row resolves to its own line through the sidecar.
    let mut lines: Vec<u32> = records.iter().map(|(line, _)| *line).collect();
    lines.sort();
    lines.dedup();
    for line in lines {
        assert!(
            text.contains(&format!("Line {line} of")),
            "line-table line {line} agrees through the sidecar:\n{text}"
        );
    }
    assert!(text.contains("Breakpoint 1 at"), "breakpoint:\n{text}");
    for (frag, i) in [
        ("g_cnt = (int)g_in;", "stop shows source"),
        ("$1 = 41", "print g_cnt"),
        ("$2 = 42", "print loc"),
        ("$3 = 41", "print g_in"),
    ] {
        assert!(text.contains(frag), "{i}:\n{text}");
    }
    // After `next` past line 8, the store ran: `g_cnt` is 42 and the
    // current line advanced to 9.
    assert!(text.contains("$4 = 42"), "print g_cnt after next:\n{text}");
    assert!(
        text.contains("Line 9 of"),
        "step advanced the line:\n{text}"
    );
}

#[test]
fn pic12_server_arg_parsing() {
    use epic_cc_gdbserver::{core_of, parse_args};
    let args = parse_args(&[
        "a.hex".to_string(),
        "a.elf".to_string(),
        "--port".to_string(),
        "1234".to_string(),
        "--device".to_string(),
        "p12f509".to_string(),
    ])
    .unwrap();
    assert_eq!(args.port, 1234);
    assert_eq!(args.device.as_deref(), Some("p12f509"));
    assert_eq!(core_of(args.device.as_deref()), Ok(Core::PicBaseline));
    assert_eq!(core_of(None), Ok(Core::Pic14));
    assert_eq!(core_of(Some("p16f1939")), Ok(Core::Pic14e));
    assert!(core_of(Some("nosuchpart")).is_err());
    assert!(parse_args(&["a.hex".to_string()]).is_err());
    assert!(parse_args(&["a.hex".to_string(), "a.elf".to_string()]).is_err());
}
