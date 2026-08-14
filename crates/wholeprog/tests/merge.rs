use wholeprog::merge;
use ir::parse;

#[test]
fn passes_single_module_through() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = merge(m);
    assert_eq!(out.funcs.len(), 1);
}

#[test]
#[should_panic]
fn rejects_empty_module() {
    merge(parse(""));
}
