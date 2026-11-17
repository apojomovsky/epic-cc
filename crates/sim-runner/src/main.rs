//! `epic-cc-sim`: the batch simulator runner binary (design 51).
//! Thin entry over the library: `--help`/`--version` first, then the
//! parse-and-run path whose exit codes are the CLI contract.

use epic_cc_sim::{parse_args, run_main, USAGE};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("epic-cc-sim {}", env!("EPIC_CC_STAMP"));
        return;
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    std::process::exit(run_main(&args));
}
