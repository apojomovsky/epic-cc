//! `epic-cc-gdbserver`: the phase-4 debugger adapter binary.
//!
//! Thin entry over the library: parse the command line, load the HEX,
//! serve one gdb session on the requested port.

use epic_cc_gdbserver::{load_program, parse_args, serve_stream};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let prog = match load_program(&args.hex_path, &args.sidecar_path) {
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
    eprintln!("epic-cc-gdbserver: waiting for gdb on {sockaddr}");
    let (stream, _) = match listener.accept() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("epic-cc-gdbserver: accept: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("epic-cc-gdbserver: debugger connected");
    serve_stream(stream, prog);
}
