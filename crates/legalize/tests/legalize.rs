use legalize::legalize;
use ir::parse;

#[test]
fn passes_8_bit_through() {
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    ret void\n");
    assert_eq!(legalize(m).funcs.len(), 1);
}
