//! PIC18 debugger acceptance (`epic-cc#1312`): the phase-4 session
//! shape rerun on the PIC18 core, proving the byte-address line table,
//! the reused typed variables, and the `Pic18Target` adapter together.
//! A real gdb session against `serve_once` with `Core::Pic18` on a
//! `p18f4550` build: `break file:line` from the sidecar `.debug_line`,
//! `continue` stops with source, `print` of a global, a local, and a
//! struct member reads the allocated address, `next` advances a line.
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
            "p18f4550",
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
        epic_cc_gdbserver::serve_once(prog, Core::Pic18, None).expect("bind server");
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
    let dir = std::env::temp_dir().join(format!("dbg1312-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pic18_session_break_print_step() {
    let dir = scratch("session");
    let src = "tests/fixtures/debug_session_pic18.c";
    let (hex, elf, lt) = compile(src, &dir);
    let prog = load_program(&hex, &elf, Core::Pic18).expect("load program");

    // The sidecar's .debug_line must agree with --line-table on every
    // mapped byte address: ask gdb for each address in one session.
    // PIC18 addresses are even (every instruction is 2+ bytes).
    let records = line_records(&lt);
    assert!(!records.is_empty(), "line table must have rows");
    assert!(
        records.iter().all(|(_, addr)| addr % 2 == 0),
        "PIC18 rows are byte addresses:\n{records:?}"
    );
    let mut cmds: Vec<String> = records
        .iter()
        .map(|(_, addr)| format!("info line *{addr:#x}"))
        .collect();
    // Then the interactive session: break, continue, typed prints,
    // stepping, run to completion. Lines 12-13 ran before the line-14
    // stop, so `loc` is 42 and `g_cnt` is still 41 there.
    cmds.extend(
        [
            "break debug_session_pic18.c:14",
            "continue",
            "print g_cnt",
            "print loc",
            "print g_p.x",
            "next",
            "info line",
            "print g_p.x",
            "next",
            "print g_p.y",
            "break debug_session_pic18.c:18",
            "continue",
            "print g_cnt",
            "continue",
            "quit",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let text = gdb_session(&elf, prog, &cmds);

    assert!(text.contains("Breakpoint 1 at"), "breakpoint:\n{text}");
    for (frag, i) in [
        ("g_p.x = loc;", "stop shows source"),
        ("$1 = 41", "print g_cnt"),
        ("$2 = 42", "print loc"),
        ("$3 = 0", "print g_p.x before the store"),
    ] {
        assert!(text.contains(frag), "{i}:\n{text}");
    }
    // After `next` past line 14, the store ran: `g_p.x` is 42 and the
    // current line advanced to 15. Past line 15, `g_p.y` is 7.
    assert!(text.contains("$4 = 42"), "print g_p.x after next:\n{text}");
    assert!(
        text.contains("Line 15 of"),
        "step advanced the line:\n{text}"
    );
    assert!(text.contains("$5 = 7"), "print g_p.y:\n{text}");
    // At line 18 every store ran: `g_cnt` is the computed total 49.
    assert!(text.contains("$6 = 49"), "print g_cnt at line 18:\n{text}");
}

#[test]
fn pic18_server_arg_parsing() {
    use epic_cc_gdbserver::{core_of, parse_args};
    let args = parse_args(&[
        "a.hex".to_string(),
        "a.elf".to_string(),
        "--port".to_string(),
        "1234".to_string(),
        "--device".to_string(),
        "p18f4550".to_string(),
    ])
    .unwrap();
    assert_eq!(args.port, 1234);
    assert_eq!(args.device.as_deref(), Some("p18f4550"));
    assert_eq!(core_of(args.device.as_deref()), Ok(Core::Pic18));
    assert_eq!(core_of(None), Ok(Core::Pic14));
    assert_eq!(core_of(Some("p16f1939")), Ok(Core::Pic14e));
    assert!(parse_args(&["a.hex".to_string()]).is_err());
    assert!(parse_args(&["a.hex".to_string(), "a.elf".to_string()]).is_err());
}
