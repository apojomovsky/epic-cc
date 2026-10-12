use ir::parse;
use wholeprog::merge;

fn names(m: &ir::Module) -> Vec<&str> {
    m.globals.iter().map(|g| g.name.as_str()).collect()
}

#[test]
fn drops_an_unreferenced_internal_global() {
    let out = merge(parse(
        "\
global live i8
global dead i16 internal
fn main(void) ()
  block 0:
    %1 = load i8 @live
    ret void
",
    ));
    assert_eq!(names(&out), vec!["live"]);
}

#[test]
fn keeps_an_unreferenced_external_global() {
    let out = merge(parse(
        "\
global poked i8
global dead i16 internal
fn main(void) ()
  block 0:
    ret void
",
    ));
    assert_eq!(names(&out), vec!["poked"]);
}

#[test]
fn keeps_internal_globals_named_by_code() {
    let out = merge(parse(
        "\
global s i8 internal
global g i8 internal
global v i8 internal
global table i8
fn main(void) ()
  block 0:
    store i8 3 @s
    %p = gep @g +0
    store i8 @v @table
    ret void
",
    ));
    assert_eq!(names(&out), vec!["s", "g", "v", "table"]);
}

#[test]
fn template_only_asm_reference_keeps_the_global() {
    let out = merge(parse(
        "\
global m_add_a i16 internal
global dead i16 internal
fn main(void) ()
  block 0:
    asm \"banksel _m_add_a\"
    ret void
",
    ));
    assert_eq!(names(&out), vec!["m_add_a"]);
}

#[test]
fn keeps_an_internal_global_reachable_from_a_live_initializer() {
    let mut m = parse(
        "\
global outer i8
global inner i8 internal
global dead i8 internal
fn main(void) ()
  block 0:
    %1 = load i8 @outer
    ret void
",
    );
    m.globals
        .iter_mut()
        .find(|g| g.name == "outer")
        .expect("outer parses")
        .refs
        .push((0, "inner".to_string(), 0));
    let out = merge(m);
    assert_eq!(names(&out), vec!["outer", "inner"]);
}

#[test]
fn drops_a_dead_chain_of_internal_globals_together() {
    let mut m = parse(
        "\
global first i8 internal
global second i8 internal
fn main(void) ()
  block 0:
    ret void
",
    );
    m.globals
        .iter_mut()
        .find(|g| g.name == "first")
        .expect("first parses")
        .refs
        .push((0, "second".to_string(), 0));
    let out = merge(m);
    assert!(out.globals.is_empty());
}

#[test]
fn select_over_globals_keeps_both_arms() {
    let out = merge(parse(
        "\
global a i8 internal
global b i8 internal
global flag i8
global dead i8 internal
fn main(void) ()
  block 0:
    %1 = select i1 @flag ptr @a ptr @b
    ret void
",
    ));
    assert_eq!(names(&out), vec!["a", "b", "flag"]);
}

#[test]
fn drops_internal_const_without_references() {
    // Flash tables drop by the same rule, including the config blob
    // (pinned only by `llvm.compiler.used`, which irparse skips): the
    // driver emits the fuse words from its own prescan, so the table
    // is dead weight (epic-cc#1164).
    let out = merge(parse(
        "\
const dead_table i8 internal
global dead_ram i8 internal
fn main(void) ()
  block 0:
    ret void
",
    ));
    assert!(out.globals.is_empty());
}

#[test]
fn module_asm_text_reference_keeps_the_global() {
    let out = merge(parse(
        "module_asm \"banksel _keepme\"\n\
global keepme i8 internal
global dead i8 internal
fn main(void) ()
  block 0:
    ret void
",
    ));
    assert_eq!(names(&out), vec!["keepme"]);
}
