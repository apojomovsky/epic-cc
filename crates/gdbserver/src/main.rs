//! `epic-cc-gdbserver`: the phase-4 debugger adapter binary.
//!
//! Thin entry over the library: parse the command line, load the HEX,
//! then accept gdb sessions in a loop so a second `target remote`,
//! a gdb `run`, or a relaunch reconnects without restarting the binary.
//! Each session reseeds a fresh simulator core; only a kill packet, a
//! fatal session error, or an accept failure ends the loop.

use epic_cc_gdbserver::{core_of, load_program, parse_args, serve_stream, USAGE};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("epic-cc-gdbserver {}", env!("EPIC_CC_STAMP"));
        return;
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let core = match core_of(args.device.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let prog = match load_program(&args.hex_path, &args.sidecar_path, core) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("epic-cc-gdbserver: {e}");
            std::process::exit(1);
        }
    };
    let sockaddr = format!("127.0.0.1:{}", args.port);
    let listener = match std::net::TcpListener::bind(&sockaddr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("epic-cc-gdbserver: bind {sockaddr}: {e}");
            std::process::exit(1);
        }
    };
    let device = args.device.as_deref().and_then(device::resolve);
    eprintln!("epic-cc-gdbserver: waiting for gdb on {sockaddr}");
    loop {
        let (stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("epic-cc-gdbserver: accept: {e}");
                std::process::exit(1);
            }
        };
        eprintln!("epic-cc-gdbserver: debugger connected");
        match serve_stream(stream, prog.clone(), core, device) {
            Ok(gdbstub::stub::DisconnectReason::Kill) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}
