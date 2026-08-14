use peephole::optimize;

#[test]
fn passes_through() {
    let asm = "    NOP\n";
    assert_eq!(optimize(asm), asm);
}
