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
/// An indirect call's `callees` list contributes one edge per candidate, so
/// the depth check and the overlay allocator see the conservative graph.
#[test]
fn indirect_call_emits_one_edge_per_candidate() {
    let m = parse(
        "fn main(void) ()\n  block entry:\n    %1 = call i8 %3() callees f0 f1\n    ret void\n\
         fn f0(void) ()\n  block entry:\n    ret void\n\
         fn f1(void) ()\n  block entry:\n    ret void\n",
    );
    let g = build(&m);
    assert!(g.edges.contains(&("main".into(), "f0".into())));
    assert!(g.edges.contains(&("main".into(), "f1".into())));
    assert_eq!(g.max_depth, 2);
}

/// A cycle through a function pointer (an indirect call whose candidate set
/// includes a function that reaches the caller) is rejected by the DFS.
#[test]
#[should_panic(expected = "recursion")]
fn recursion_through_function_pointer_detected() {
    let m = parse(
        "fn f(void) ()\n  block entry:\n    %1 = call void %3() callees g\n    ret void\n\
         fn g(void) ()\n  block entry:\n    call void @f()\n    ret void\n",
    );
    let _ = build(&m);
}

/// A back edge reports the source location of the call that closes the
/// cycle, resolved from the module's debug info (epic-cc#175).
#[test]
#[should_panic(expected = "f.c:3:7: callgraph: recursion detected (call cycle involving f)")]
fn recursion_names_the_call_site() {
    let mut m = parse("fn f(void) ()\n  block entry:\n    call void @f()\n    ret void\n");
    for f in &mut m.funcs {
        for b in &mut f.blocks {
            for i in &mut b.insts {
                if let ir::Inst::Call(c) = i {
                    c.loc = Some(ir::SrcLoc {
                        file: "f.c".to_string(),
                        line: 3,
                        col: 7,
                    });
                }
            }
        }
    }
    let _ = build(&m);
}
