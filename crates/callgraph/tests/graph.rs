use callgraph::{build, check_depth};
use ir::parse;

#[test]
fn single_function_has_no_edges() {
    let m = parse("fn main() -> void\n  block entry:\n    ret void\n");
    let g = build(&m);
    assert!(g.edges.is_empty());
    assert_eq!(g.max_depth, 1);
}

#[test]
fn call_edge_and_depth() {
    let m = parse(
        "fn main() -> void\n  block entry:\n    call void @add()\n    ret void\n\
         fn add() -> void\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    assert!(g.edges.contains(&("main".into(), "add".into())));
    assert_eq!(g.max_depth, 2);
}

#[test]
#[should_panic(expected = "recursion")]
fn recursion_detected() {
    let m = parse(
        "fn f() -> void\n  block entry:\n    call void @g()\n    ret void\n\
         fn g() -> void\n  block entry:\n    call void @f()\n    ret void\n",
    );
    let _ = build(&m);
}

#[test]
fn long_chain_depth_check() {
    let m = parse(
        "fn f() -> void\n  block entry:\n    call void @g()\n    ret void\n\
         fn g() -> void\n  block entry:\n    call void @h()\n    ret void\n\
         fn h() -> void\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    assert_eq!(g.max_depth, 3);
    check_depth(&g, 8);
    assert!(std::panic::catch_unwind(|| check_depth(&g, 2)).is_err());
}
