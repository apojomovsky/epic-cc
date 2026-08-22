use ir::parse;
use wholeprog::merge;

const RESOLVED: &str = "\
fn main(void) ()
  block 0:
    %1 = call i8 @helper(i8 3)
    ret void
fn helper(i8) (0=i8)
  block 0:
    ret i8 %0
";

#[test]
fn accepts_a_resolved_module() {
    let out = merge(parse(RESOLVED));
    assert_eq!(out.funcs.len(), 2);
}

#[test]
#[should_panic(expected = "undefined symbols: from_b")]
fn rejects_a_call_with_no_definition() {
    merge(parse(
        "\
fn main(void) ()
  block 0:
    %1 = call i8 @from_b(i8 3)
    ret void
",
    ));
}

#[test]
#[should_panic(expected = "undefined symbols: alpha, beta")]
fn lists_every_undefined_symbol_sorted() {
    // Called in the order beta, alpha; reported sorted, because a BTreeSet
    // makes the diagnostic stable across runs.
    merge(parse(
        "\
fn main(void) ()
  block 0:
    %1 = call i8 @beta(i8 1)
    %2 = call i8 @alpha(i8 2)
    ret void
",
    ));
}

#[test]
#[should_panic(expected = "exactly one `main`")]
fn rejects_a_module_with_no_main() {
    merge(parse(
        "\
fn helper(i8) (0=i8)
  block 0:
    ret i8 %0
",
    ));
}
