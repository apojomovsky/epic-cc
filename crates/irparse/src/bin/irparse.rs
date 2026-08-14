use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = irparse::parse_ll(&src);
    fs::write(&args[2], ir::serialize(&m)).expect("write output");
}
