//! CLI: assemble a PIC14 `.asm` file into Intel HEX.
//!
//! Usage: `asm <in.asm> <out.hex>`

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(in_path), Some(out_path)) = (args.next(), args.next()) else {
        eprintln!("usage: asm <in.asm> <out.hex>");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(&in_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("asm: cannot read {in_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let hex = asm::assemble_file_to_hex(&device::PIC16F877A, &src);
    if let Err(e) = std::fs::write(&out_path, hex) {
        eprintln!("asm: cannot write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
