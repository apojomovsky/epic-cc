//! CLI: factor a PIC18 `.asm` listing (the stage boundary, for bisecting).
//!
//! Usage: `outline <in.asm> <out.asm>`

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(in_path), Some(out_path)) = (args.next(), args.next()) else {
        eprintln!("usage: outline <in.asm> <out.asm>");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(&in_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("outline: cannot read {in_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let out = outline::factor(&src, &outline::Options::default());
    if let Err(e) = std::fs::write(&out_path, out) {
        eprintln!("outline: cannot write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
