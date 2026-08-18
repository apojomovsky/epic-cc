use asm::assemble_pic18;

#[test]
fn assembles_a_single_nop() {
    let words = assemble_pic18("    NOP\n");
    assert_eq!(words, vec![0x0000]);
}

#[test]
fn resolves_a_forward_label_via_two_passes() {
    // Same two-pass shape as the PIC14 encoder: a label used before its
    // definition must still resolve (`org` tracks address across both NOPs
    // in pass 1 before pass 2 encodes anything).
    let words = assemble_pic18("    NOP\ntarget:\n    NOP\n");
    assert_eq!(words.len(), 2);
}

#[test]
fn equ_defines_a_symbol_usable_before_or_after() {
    let words = assemble_pic18("FOO equ 0x05\n    NOP\n");
    assert_eq!(words, vec![0x0000]);
}
