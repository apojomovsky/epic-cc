use std::fs;
use legalize::legalize;
use ir::{parse, serialize};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = legalize(parse(&src));
    fs::write(&args[2], serialize(&m)).expect("write output");
}
