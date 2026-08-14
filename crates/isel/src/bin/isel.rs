use isel::select;
use std::collections::HashMap;
use std::fs;

/// `isel <in.ir> <in.map> <out.asm>`
///
/// The address map is a text file with `global <name> 0xNN` lines (produced
/// by the `alloc` stage).
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let map = fs::read_to_string(&args[2]).expect("read map");
    let addrs = parse_map(&map);
    let asm = select(&ir::parse(&src), &addrs);
    fs::write(&args[3], asm).expect("write output");
}

fn parse_map(text: &str) -> HashMap<String, u8> {
    let mut addrs = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().expect("map entry");
        if kw != "global" {
            panic!("isel: unexpected map line: {line}");
        }
        let name = it.next().expect("map name").to_string();
        let addr = it
            .next()
            .and_then(|h| u8::from_str_radix(h.trim_start_matches("0x"), 16).ok())
            .unwrap_or_else(|| panic!("isel: bad address in map line: {line}"));
        addrs.insert(name, addr);
    }
    addrs
}
