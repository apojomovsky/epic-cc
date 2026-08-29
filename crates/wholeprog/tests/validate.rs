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
    // Called in the order beta, alpha; reported sorted, because a BTreeMap
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

/// An `llvm.*` intrinsic call is `declare`d by clang, never defined here;
/// wholeprog must not treat it as an undefined user symbol. legalize lowers
/// every supported one and panics loudly on an unknown.
#[test]
fn accepts_an_intrinsic_call() {
    let out = merge(parse(
        "\
fn main(void) ()
  block 0:
    %1 = call i16 @llvm.smax.i16(i16 %2, i16 %3)
    ret void
",
    ));
    assert_eq!(out.funcs.len(), 1);
}

/// With debug locations on the calls, each undefined symbol also names the
/// call sites that reference it (epic-cc#175).
#[test]
#[should_panic(expected = "undefined symbols: from_b (called at main.c:3:3)")]
fn undefined_symbol_names_its_call_site() {
    let mut m = parse(
        "\
fn main(void) ()
  block 0:
    %1 = call i8 @from_b(i8 3)
    ret void
",
    );
    for f in &mut m.funcs {
        for b in &mut f.blocks {
            for i in &mut b.insts {
                if let ir::Inst::Call(c) = i {
                    c.loc = Some(ir::SrcLoc {
                        file: "main.c".to_string(),
                        line: 3,
                        col: 3,
                    });
                }
            }
        }
    }
    merge(m);
}
