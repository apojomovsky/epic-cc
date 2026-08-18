use banking::assign_banks;
use std::fs;

/// `banking <in.asm> <out.asm>`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let asm = assign_banks(&device::PIC16F877A, &src);
    fs::write(&args[2], asm).expect("write output");
}
