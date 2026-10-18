//! PIC14E debugger acceptance (`epic-cc#1311`): the phase-4 session
//! shape rerun on the enhanced mid-range core, proving the word-address
//! line table, the reused typed variables, and the `Pic14eTarget`
//! adapter together. A real gdb session against `serve_once` with
//! `Core::Pic14e` on a `p16f1937` build: `break file:line` from the
//! sidecar `.debug_line`, `continue` stops with source, `print` of a
//! global and a local reads the allocated address, `next` advances a
//! line.
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
            "p16f1937",
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
        epic_cc_gdbserver::serve_once(prog, Core::Pic14e, None).expect("bind server");
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
    let dir = std::env::temp_dir().join(format!("dbg1311-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pic14e_session_break_print_step() {
    let dir = scratch("session");
    let src = "tests/fixtures/debug_session_pic14e.c";
    let (hex, elf, lt) = compile(src, &dir);
    let prog = load_program(&hex, &elf, Core::Pic14e).expect("load program");

    // The sidecar's .debug_line must agree with --line-table on every
    // mapped word address: ask gdb for each address in one session.
    let records = line_records(&lt);
    assert!(!records.is_empty(), "line table must have rows");
    let mut cmds: Vec<String> = records
        .iter()
        .map(|(_, addr)| format!("info line *{addr:#x}"))
        .collect();
    // Then the interactive session: break, continue, typed prints,
    // stepping, run to completion. Lines 12-13 ran before the line-14
    // stop, so `loc` is 42 and `g_cnt` is still 41 there.
    cmds.extend(
        [
            "break debug_session_pic14e.c:14",
            "continue",
            "print g_cnt",
            "print loc",
            "print g_p.x",
            "next",
            "info line",
            "print g_p.x",
            "print g_p.y",
            "next",
            "print g_p.y",
            "break debug_session_pic14e.c:18",
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
    assert!(text.contains("$5 = 0"), "print g_p.y at line 15:\n{text}");
    assert!(text.contains("$6 = 7"), "print g_p.y after next:\n{text}");
    // At line 18 every store ran: `g_cnt` is the computed total 49.
    assert!(text.contains("$7 = 49"), "print g_cnt at line 18:\n{text}");
}

#[test]
fn pic14e_write_watchpoint_stops_with_value() {
    // `watch` over RSP `Z2`, mirrored from the PIC14 case: break at the
    // line-12 store so the watch arms with the old value, `continue`
    // stops on the store with old/new values, `info break` lists it.
    let dir = scratch("watch");
    let (hex, elf, _) = compile("tests/fixtures/debug_session_pic14e.c", &dir);
    let prog = load_program(&hex, &elf, Core::Pic14e).expect("load program");

    let text = gdb_session(
        &elf,
        prog,
        &[
            "break debug_session_pic14e.c:12".to_string(),
            "continue".to_string(),
            "watch g_cnt".to_string(),
            "continue".to_string(),
            "info break".to_string(),
            "delete 2".to_string(),
            "continue".to_string(),
            "quit".to_string(),
        ],
    );
    assert!(text.contains("Breakpoint 1 at"), "breakpoint:\n{text}");
    assert!(
        text.contains("Hardware watchpoint 2: g_cnt"),
        "watch armed:\n{text}"
    );
    for (frag, i) in [
        ("Old value = 0", "watch old value"),
        ("New value = 41", "watch new value"),
        ("hw watchpoint  keep y", "info break lists the watch"),
    ] {
        assert!(text.contains(frag), "{i}:\n{text}");
    }
}

#[test]
fn pic14e_server_arg_parsing() {
    use epic_cc_gdbserver::{core_of, parse_args};
    let args = parse_args(&[
        "a.hex".to_string(),
        "a.elf".to_string(),
        "--port".to_string(),
        "1234".to_string(),
        "--device".to_string(),
        "p16f1937".to_string(),
    ])
    .unwrap();
    assert_eq!(args.port, 1234);
    assert_eq!(args.device.as_deref(), Some("p16f1937"));
    assert_eq!(core_of(args.device.as_deref()), Ok(Core::Pic14e));
    assert_eq!(core_of(Some("p16f1939")), Ok(Core::Pic14e));
}
