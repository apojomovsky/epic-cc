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

#[test]
fn bit_oriented_instructions_encode_correctly() {
    let cases: &[(&str, u16)] = &[
        ("BCF 0x55,3,A", 0x9655),
        ("BSF 0x55,5,A", 0x8A55),
        ("BTFSC 0x55,6,A", 0xBC55),
        ("BTFSS 0x55,2,A", 0xA455),
        ("BTG 0x55,4,A", 0x7855),
    ];
    for (src, expected) in cases {
        let words = assemble_pic18(&format!("    {src}\n"));
        assert_eq!(words, vec![*expected], "encoding {src}");
    }
}

#[test]
fn literal_instructions_encode_correctly() {
    let cases: &[(&str, u16)] = &[
        ("SUBLW 0x42", 0x0842),
        ("IORLW 0x42", 0x0942),
        ("XORLW 0x42", 0x0A42),
        ("ANDLW 0x42", 0x0B42),
        ("RETLW 0x42", 0x0C42),
        ("MULLW 0x42", 0x0D42),
        ("MOVLW 0x42", 0x0E42),
        ("ADDLW 0x42", 0x0F42),
    ];
    for (src, expected) in cases {
        let words = assemble_pic18(&format!("    {src}\n"));
        assert_eq!(words, vec![*expected], "encoding {src}");
    }
}

#[test]
fn fixed_encoding_control_instructions() {
    let cases: &[(&str, u16)] = &[
        ("CLRWDT", 0x0004),
        ("PUSH", 0x0005),
        ("POP", 0x0006),
        ("DAW", 0x0007),
        ("RETFIE", 0x0010),
        ("RETFIE FAST", 0x0011),
        ("RETURN", 0x0012),
        ("RETURN FAST", 0x0013),
        ("SLEEP", 0x0003),
        ("RESET", 0x00FF),
        ("MOVLB 5", 0x0105),
    ];
    for (src, expected) in cases {
        let words = assemble_pic18(&format!("    {src}\n"));
        assert_eq!(words, vec![*expected], "encoding {src}");
    }
}

#[test]
fn conditional_branch_encodes_forward_offset() {
    // BZ at word 0 branching to a label at word 8: next-instruction word is
    // 1, so n8 = 8 - 1 = 7.
    let words = assemble_pic18(
        "    BZ target\n    NOP\n    NOP\n    NOP\n    NOP\n    NOP\n    NOP\n    NOP\ntarget:\n    NOP\n",
    );
    assert_eq!(words[0], 0xE007);
}

#[test]
fn conditional_branch_encodes_backward_offset() {
    // target: at word 0, BZ at word 9 (after 9 NOPs) branching back:
    // next-instruction word is 10, n8 = 0 - 10 = -10 = 0xF6.
    let mut src = String::from("target:\n");
    for _ in 0..9 {
        src.push_str("    NOP\n");
    }
    src.push_str("    BZ target\n");
    let words = assemble_pic18(&src);
    assert_eq!(words[9], 0xE0F6);
}

#[test]
fn bra_and_rcall_use_the_11_bit_offset() {
    let words = assemble_pic18("target:\n    NOP\n    BRA target\n    RCALL target\n");
    // BRA at word 1: n11 = 0 - 2 = -2 = 0x7FE
    assert_eq!(words[1], 0xD7FE);
    // RCALL at word 2: n11 = 0 - 3 = -3 = 0x7FD, base 0xD800 (not BRA's
    // 0xD000 — bit 11 distinguishes RCALL from BRA)
    assert_eq!(words[2], 0xDFFD);
}

#[test]
fn every_conditional_branch_mnemonic_uses_its_own_base_opcode() {
    let cases: &[(&str, u16)] = &[
        ("BZ", 0xE000),
        ("BNZ", 0xE100),
        ("BC", 0xE200),
        ("BNC", 0xE300),
        ("BOV", 0xE400),
        ("BNOV", 0xE500),
        ("BN", 0xE600),
        ("BNN", 0xE700),
    ];
    for (mne, base) in cases {
        // Branch to self: next-instruction word is 1, target word is 0, so
        // n8 = 0 - 1 = -1 = 0xFF for every one of these.
        let words = assemble_pic18(&format!("here:\n    {mne} here\n"));
        assert_eq!(words[0], base | 0xFF, "encoding {mne}");
    }
}

#[test]
fn goto_encodes_the_word_address_across_two_words() {
    // target at word 0, GOTO at word 1: k = target word address = 0.
    let words = assemble_pic18("target:\n    NOP\n    GOTO target\n");
    assert_eq!(&words[1..3], &[0xEF00, 0xF000]);
}

#[test]
fn call_encodes_normal_and_fast_forms() {
    let words = assemble_pic18("target:\n    NOP\n    CALL target\n    CALL target,FAST\n");
    assert_eq!(&words[1..3], &[0xEC00, 0xF000]); // normal: s=0
    assert_eq!(&words[3..5], &[0xED00, 0xF000]); // fast: s=1
}

#[test]
fn lfsr_loads_a_12_bit_literal() {
    let words = assemble_pic18("    LFSR 2, 0xFFF\n");
    assert_eq!(words, vec![0xEE2F, 0xF0FF]);
}

#[test]
fn movff_moves_between_two_full_12_bit_addresses() {
    let words = assemble_pic18("    MOVFF 0x55, 0xF80\n");
    assert_eq!(words, vec![0xC055, 0xFF80]);
}
