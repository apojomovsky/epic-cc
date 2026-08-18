use isel::{parse_map, select};
use std::fs;

/// `isel <in.ir> <in.map> <out.asm>`
///
/// The address map is a text file with `global <name> 0xNN`,
/// `local <func> <name> 0xNN`, and `const <name>` (no address — flash)
/// lines (produced by the `alloc` stage). Locals are keyed
/// `{func}::{name}`, matching the keys isel looks up.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let map = fs::read_to_string(&args[2]).expect("read map");
    let addrs = parse_map(&map);
    let asm = select(&device::PIC16F877A, &ir::parse(&src), &addrs);
    fs::write(&args[3], asm).expect("write output");
}
