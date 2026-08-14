use alloc::allocate;
use ir::parse;

#[test]
fn globals_get_bank0_addresses() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = allocate(m);
    assert_eq!(out.globals[0].addr, Some(0x20));
    assert_eq!(out.globals[1].addr, Some(0x21));
}
