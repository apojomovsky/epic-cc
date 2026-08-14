use std::fs;
use wholeprog::merge;
use ir::{parse, serialize};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = merge(parse(&src));
    fs::write(&args[2], serialize(&m)).expect("write output");
}
