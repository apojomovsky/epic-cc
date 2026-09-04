use schedule::schedule;
use std::fs;

/// `schedule <in.asm> <out.asm>`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let asm = schedule(&device::PIC16F877A, &src);
    fs::write(&args[2], asm).expect("write output");
}
