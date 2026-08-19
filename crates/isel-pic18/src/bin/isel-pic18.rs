use std::fs;

/// `isel-pic18 <in.ir> <in.map> <out.asm>`
///
/// The address map is a text file with `global <name> 0xNN`,
/// `local <func> <name> 0xNN`, and `const <name>` (no address — flash)
/// lines (produced by the `alloc` stage). Locals are keyed
/// `{func}::{name}`, matching the keys isel-pic18 looks up. The parser
/// itself has nothing PIC18-specific about it, so it's reused from
/// `iselcore` (shared with `isel`) rather than duplicated here.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let map = fs::read_to_string(&args[2]).expect("read map");
    let addrs = iselcore::parse_map(&map);
    let asm = isel_pic18::select(&device::PIC18F4550, &ir::parse(&src), &addrs);
    fs::write(&args[3], asm).expect("write output");
}
