use std::fs;

use alloc::{allocate, map_text};
use ir::parse;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input IR");
    let edges = fs::read_to_string(&args[2]).expect("read callgraph edges");
    let m = parse(&src);
    let layout = allocate(&device::PIC16F877A, &m, &edges);
    fs::write(&args[3], map_text(&layout)).expect("write map");
}
