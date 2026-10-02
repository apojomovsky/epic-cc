use isel::{parse_map, select_with_locs};
use std::fs;

/// `isel <in.ir> <in.map> <out.asm>`
///
/// The address map is a text file with `global <name> 0xNN`,
/// `local <func> <name> 0xNN`, `const <name>` (no address, flash), and
/// `staged <name>` (stage through the shared buffer) lines (produced by
/// the `alloc` stage). Locals are keyed `{func}::{name}`, matching the
/// keys isel looks up.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let map = fs::read_to_string(&args[2]).expect("read map");
    let (addrs, staged) = parse_map(&map);
    let (asm, _, _) = select_with_locs(
        &device::PIC16F877A,
        &ir::parse(&src),
        &addrs,
        &staged,
        &isel::ConstPool::empty(),
    );
    fs::write(&args[3], asm).expect("write output");
}
