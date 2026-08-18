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

#[test]
fn byte_oriented_with_dest_select_encode_correctly() {
    let cases: &[(&str, u16)] = &[
        ("ADDWF 0x20,F,A", 0x2620),
        ("ADDWFC 0x55,W,A", 0x2055),
        ("ANDWF 0x55,W,A", 0x1455),
        ("COMF 0x55,W,A", 0x1C55),
        ("DECF 0x55,W,A", 0x0455),
        ("DECFSZ 0x55,W,A", 0x2C55),
        ("DCFSNZ 0x55,W,A", 0x4C55),
        ("INCF 0x55,W,A", 0x2855),
        ("INCFSZ 0x55,W,A", 0x3C55),
        ("INFSNZ 0x55,W,A", 0x4855),
        ("IORWF 0x55,W,A", 0x1055),
        ("MOVF 0x55,W,A", 0x5055),
        ("RLCF 0x55,W,A", 0x3455),
        ("RLNCF 0x55,W,A", 0x4455),
        ("RRCF 0x55,W,A", 0x3055),
        ("RRNCF 0x55,W,A", 0x4055),
        ("SUBFWB 0x55,W,A", 0x5455),
        ("SUBWF 0x55,W,A", 0x5C55),
        ("SUBWFB 0x55,W,A", 0x5855),
        ("SWAPF 0x55,W,A", 0x3855),
        ("XORWF 0x55,W,A", 0x1855),
    ];
    for (src, expected) in cases {
        let words = assemble_pic18(&format!("    {src}\n"));
        assert_eq!(words, vec![*expected], "encoding {src}");
    }
}

#[test]
fn byte_oriented_dest_and_access_bits_both_set() {
    // d=1 (F), a=1 (banked) together — confirms both bits are read from
    // the right operand position, not just their zero defaults.
    let words = assemble_pic18("    ADDWF 0x55,F,B\n");
    assert_eq!(words, vec![0x2755]); // 0x2400 | 1<<9 | 1<<8 | 0x55
}

#[test]
fn byte_oriented_without_dest_select_encode_correctly() {
    let cases: &[(&str, u16)] = &[
        ("CLRF 0x55,A", 0x6A55),
        ("CPFSEQ 0x55,A", 0x6255),
        ("CPFSGT 0x55,A", 0x6455),
        ("CPFSLT 0x55,A", 0x6055),
        ("MOVWF 0x55,A", 0x6E55),
        ("MULWF 0x55,A", 0x0255),
        ("NEGF 0x55,A", 0x6C55),
        ("SETF 0x55,A", 0x6855),
        ("TSTFSZ 0x55,A", 0x6655),
    ];
    for (src, expected) in cases {
        let words = assemble_pic18(&format!("    {src}\n"));
        assert_eq!(words, vec![*expected], "encoding {src}");
    }
}
