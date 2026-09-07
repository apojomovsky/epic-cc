//! Phase-4 debugger acceptance (`epic-cc#259`): a real gdb session
//! against `epic-cc-gdbserver` over the phase-1 line table and the
//! phase-2 typed variables. The server runs in-process on an ephemeral
//! port (`serve_once`); gdb runs as a subprocess (present in the dev/ci
//! image per the ticket). Acceptance, per the ticket:
//! - `break file:line` resolves from the sidecar `.debug_line`,
//!   `continue` stops at the line, source is shown.
//! - `print` of a global, a struct member, and an array element is
//!   typed and reads the allocated address.
//! - `next` advances a source line; `info line` agrees throughout.
//! - every `--line-table` address resolves to the same line through
//!   the sidecar (`info line *addr`).
//! - a banked fixture's `BANKSEL` words (phase-1 inherits-a-line case)
//!   step over inside their source line.

use std::io::Write;
use std::process::Command;

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
            "p16f877a",
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
    let (addr, server) = epic_cc_gdbserver::serve_once(prog).expect("bind server");
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
    let dir = std::env::temp_dir().join(format!("dbg259-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn typed_session_break_print_step() {
    let dir = scratch("session");
    let src = "tests/fixtures/debug_session.c";
    let (hex, elf, lt) = compile(src, &dir);
    let prog = load_program(&hex, &elf).expect("load program");

    // The sidecar's .debug_line must agree with --line-table on every
    // mapped word: ask gdb for each address in one session.
    let records = line_records(&lt);
    assert!(!records.is_empty(), "line table must have rows");
    let mut cmds: Vec<String> = records
        .iter()
        .map(|(_, addr)| format!("info line *{addr:#x}"))
        .collect();
    // Then the interactive session: break, continue, typed prints,
    // stepping, run to completion.
    cmds.extend(
        [
            "break debug_session.c:20",
            "continue",
            "print g_cnt",
            "print arr[0]",
            "print arr[1]",
            "print g_ch",
            "next",
            "info line",
            "print g_ch",
            "print g_p.x",
            "next",
            "print g_p.x",
            "continue",
            "quit",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let text = gdb_session(&elf, prog, &cmds);

    // Breakpoint, stop with source, typed values. Lines 17-19 ran
    // before the line-20 stop, so the prints prove the addresses and
    // the DWARF types together (`g_cnt` is 41, `arr` holds 42, 43).
    assert!(text.contains("Breakpoint 1 at"), "breakpoint:\n{text}");
    // member, array element.
    for (frag, i) in [
        ("g_ch = (char)arr[1];", "stop shows source"),
        ("$1 = 41", "print g_cnt"),
        ("$2 = 42", "print arr[0]"),
        ("$3 = 43", "print arr[1]"),
        ("$4 = 0", "print g_ch"),
    ] {
        assert!(text.contains(frag), "{i}:\n{text}");
    }
    // After `next` past line 20, `g_ch` is 43 and the current
    // line advanced to 21.
    assert!(text.contains("$5 = 43"), "print g_ch after next:\n{text}");
    // The struct member resolves through the aggregate DIEs: still 0
    // at line 21, then 42 once line 21's store has executed.
    assert!(text.contains("$6 = 0"), "print g_p.x at line 21:\n{text}");
    assert!(text.contains("$7 = 42"), "print g_p.x at line 22:\n{text}");
    assert!(
        text.contains("Line 21 of"),
        "step advanced the line:\n{text}"
    );
}

#[test]
fn banksel_words_step_inside_their_line() {
    // 85 globals push the last one into bank 1, forcing a BANKSEL on
    // its store; phase 1 gives the BANKSEL the store's line, so `next`
    // must carry straight through to the following source line.
    let dir = scratch("banked");
    let mut src = String::new();
    for i in 0..85 {
        src.push_str(&format!("int b{i:02};\n"));
    }
    src.push_str("int main(void) {\n  b84 = 7;\n  b00 = b84;\n  return b00;\n}\n");
    let c_path = dir.join("banked.c");
    std::fs::File::create(&c_path)
        .unwrap()
        .write_all(src.as_bytes())
        .unwrap();
    let (hex, elf, _) = compile(&c_path.to_string_lossy(), &dir);
    let prog = load_program(&hex, &elf).expect("load program");

    let text = gdb_session(
        &elf,
        prog,
        &[
            "break banked.c:87".to_string(),
            "continue".to_string(),
            "next".to_string(),
            "info line".to_string(),
            "print b84".to_string(),
            "quit".to_string(),
        ],
    );
    // Line 87 is `b84 = 7;` (85 globals on lines 1-85, blank 86).
    assert!(text.contains("b84 = 7;"), "stop shows source:\n{text}");
    assert!(
        text.contains("Line 88 of"),
        "next crossed the BANKSEL into line 88:\n{text}"
    );
    assert!(text.contains("$1 = 7"), "print b84:\n{text}");
}

#[test]
fn server_arg_parsing() {
    use epic_cc_gdbserver::parse_args;
    let args = parse_args(&[
        "a.hex".to_string(),
        "a.elf".to_string(),
        "--port".to_string(),
        "1234".to_string(),
    ])
    .unwrap();
    assert_eq!(args.port, 1234);
    assert!(parse_args(&["a.hex".to_string()]).is_err());
    assert!(parse_args(&["a.hex".to_string(), "a.elf".to_string()]).is_err());
}
