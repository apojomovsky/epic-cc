use callgraph::{build, check_depth, edges_text};
use ir::parse;

#[test]
fn single_function_has_no_edges() {
    let m = parse("fn main(void) ()\n  block entry:\n    ret void\n");
    let g = build(&m);
    assert!(g.edges.is_empty());
    assert_eq!(g.max_depth, 1);
}

#[test]
fn call_edge_and_depth() {
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @add()\n    ret void\n\
         fn add(void) ()\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    assert!(g.edges.contains(&("main".into(), "add".into())));
    assert_eq!(g.max_depth, 2);
}

#[test]
#[should_panic(expected = "recursion")]
fn recursion_detected() {
    let m = parse(
        "fn f(void) ()\n  block entry:\n    call void @g()\n    ret void\n\
         fn g(void) ()\n  block entry:\n    call void @f()\n    ret void\n",
    );
    let _ = build(&m);
}

#[test]
fn long_chain_depth_check() {
    let m = parse(
        "fn f(void) ()\n  block entry:\n    call void @g()\n    ret void\n\
         fn g(void) ()\n  block entry:\n    call void @h()\n    ret void\n\
         fn h(void) ()\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    assert_eq!(g.max_depth, 3);
    check_depth(&g, 8);
    assert!(std::panic::catch_unwind(|| check_depth(&g, 2)).is_err());
}

#[test]
fn edges_text_parseable_format() {
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @a()\n    call void @b()\n    ret void\n\
         fn a(void) ()\n  block entry:\n    ret void\n\
         fn b(void) ()\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    let text = edges_text(&g);
    assert!(text.contains("edge main a\n"));
    assert!(text.contains("edge main b\n"));
    assert!(text.contains("depth 2\n"));
}
