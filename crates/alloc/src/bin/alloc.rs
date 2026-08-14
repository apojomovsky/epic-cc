use std::fs;
use alloc::{address_map, allocate};
use ir::{parse, serialize};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = allocate(parse(&src));
    fs::write(&args[2], serialize(&m)).expect("write output");
    fs::write(&args[3], address_map(&m)).expect("write map");
}
