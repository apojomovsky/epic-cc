use callgraph::build;
use ir::parse;

#[test]
fn single_function_has_no_edges() {
    let m = parse("fn main() -> void\n  block entry:\n    ret void\n");
    let g = build(&m);
    assert!(g.edges.is_empty());
    assert_eq!(g.max_depth, 1);
}
