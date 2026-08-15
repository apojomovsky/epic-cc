use std::fs;
use callgraph::{build, check_depth, edges_text};
use ir::parse;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = parse(&src);
    let g = build(&m);
    check_depth(&g, 8);
    let mut out = edges_text(&g);
    for f in &m.funcs {
        out.push_str(&format!("fn {}\n", f.name));
    }
    fs::write(&args[2], out).expect("write output");
}
