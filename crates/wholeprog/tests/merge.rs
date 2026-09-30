use ir::parse;
use wholeprog::merge;

#[test]
fn passes_single_module_through() {
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    ret void\n");
    let out = merge(m);
    assert_eq!(out.funcs.len(), 1);
}

#[test]
#[should_panic]
fn rejects_empty_module() {
    merge(parse(""));
}

#[test]
fn maps_lone_mangled_cpp_entry_to_main() {
    let out = merge(parse(
        "fn _Z4mainv(void) ()\n  block entry:\n    ret void\n",
    ));
    assert_eq!(out.funcs.len(), 1);
    assert_eq!(out.funcs[0].name, "main");
}

#[test]
#[should_panic(expected = "expected exactly one `main`")]
fn rejects_plain_main_alongside_mangled_entry() {
    merge(parse(
        "fn main(void) ()\n  block entry:\n    ret void\nfn _Z4mainv(void) ()\n  block entry:\n    ret void\n",
    ));
}
