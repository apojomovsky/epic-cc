use device::PIC16F877A;
use schedule::{classify, regions, schedule, Line};

#[test]
fn identity_transform_is_lossless() {
    let asm = "\
main:
    MOVF 0x20, W
    MOVWF 0x27
    ANDWF 0x28, W
    BTFSC STATUS, 2
    GOTO main_L2
    CALL helper
main_L2:
    RETURN
";
    assert_eq!(schedule(&PIC16F877A, asm), asm);
}

#[test]
fn identity_transform_preserves_a_missing_trailing_newline() {
    // isel's own raw output has no trailing newline (found via the full
    // fuzz corpus, seed 128): always appending one after the last line
    // silently grew the text by a phantom blank line that crates/asm
    // does not parse the same as no trailing line at all.
    let asm = "    MOVF 0x20, W\n    MOVWF 0x21\n    end";
    assert_eq!(schedule(&PIC16F877A, asm), asm);
}

#[test]
fn classifies_bank_the_same_way_banking_does() {
    // Cross-checked directly against banking::operand_bank rather than a
    // hand-picked expectation, so the two passes can never silently
    // disagree about which bank an operand needs (ADR-027).
    let asm = "    MOVF 0x80, W\n    ANDWF 0x188, W\n    MOVWF 0x20\n";
    let lines = classify(&PIC16F877A, asm);
    let banks: Vec<Option<u8>> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Insn(i) => Some(i.bank),
            _ => None,
        })
        .collect();
    assert_eq!(
        banks,
        vec![
            banking::operand_bank(&PIC16F877A, "MOVF", &["MOVF", "0x80,", "W"]),
            banking::operand_bank(&PIC16F877A, "ANDWF", &["ANDWF", "0x188,", "W"]),
            banking::operand_bank(&PIC16F877A, "MOVWF", &["MOVWF", "0x20"]),
        ]
    );
}

#[test]
fn region_splits_at_a_label() {
    let asm = "    MOVWF 0x20\nl1:\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    let rs = regions(&lines);
    assert_eq!(rs, vec![0..1, 2..3], "label at index 1 splits the regions");
}

#[test]
fn region_splits_at_a_call() {
    let asm = "    MOVWF 0x20\n    CALL helper\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    let rs = regions(&lines);
    assert_eq!(rs, vec![0..1, 2..3], "CALL at index 1 splits the regions");
}

#[test]
fn region_splits_at_a_terminator() {
    let asm = "    MOVWF 0x20\n    GOTO l1\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    let rs = regions(&lines);
    assert_eq!(rs, vec![0..1, 2..3], "GOTO at index 1 splits the regions");
}

#[test]
fn region_splits_at_an_asm_barrier() {
    let asm = "    MOVWF 0x20\n; --- asm start ---\n    NOP\n; --- asm end ---\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    let rs = regions(&lines);
    assert_eq!(
        rs,
        vec![0..1, 4..5],
        "the verbatim block and its markers are all barriers"
    );
}

#[test]
fn region_splits_at_a_directive() {
    let asm = "    MOVWF 0x20\n    .align 256\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    let rs = regions(&lines);
    assert_eq!(rs, vec![0..1, 2..3], "a directive splits the regions too");
}

#[test]
fn skip_op_marks_its_immediate_successor_as_a_skip_target() {
    let asm = "    BTFSC STATUS, 2\n    MOVWF 0x20\n";
    let lines = classify(&PIC16F877A, asm);
    match &lines[0] {
        Line::Insn(i) => assert!(i.is_skip && !i.is_skip_target),
        other => panic!("expected an Insn, got {other:?}"),
    }
    match &lines[1] {
        Line::Insn(i) => assert!(i.is_skip_target, "the skip's immediate successor"),
        other => panic!("expected an Insn, got {other:?}"),
    }
}

#[test]
fn a_line_two_after_a_skip_is_not_a_skip_target() {
    let asm = "    BTFSC STATUS, 2\n    MOVWF 0x20\n    MOVWF 0x21\n";
    let lines = classify(&PIC16F877A, asm);
    match &lines[2] {
        Line::Insn(i) => assert!(!i.is_skip_target),
        other => panic!("expected an Insn, got {other:?}"),
    }
}

#[test]
fn a_status_bit_clear_is_a_flags_write_even_off_the_rp_bits() {
    // BCF STATUS,0 clears Carry directly: not one of banking's RP bits,
    // but still a flags write schedule must never let a move cross.
    let asm = "    BCF STATUS, 0\n";
    let lines = classify(&PIC16F877A, asm);
    match &lines[0] {
        Line::Insn(i) => assert!(i.writes_flags && i.reads_flags),
        other => panic!("expected an Insn, got {other:?}"),
    }
}

#[test]
fn an_unrecognized_mnemonic_is_opaque_not_guessed() {
    let asm = "    SLEEP\n";
    let lines = classify(&PIC16F877A, asm);
    assert!(matches!(lines[0], Line::Opaque(_)));
}

#[test]
fn dest_file_form_reads_and_writes_the_same_address() {
    let asm = "    RLF 0x20, F\n";
    let lines = classify(&PIC16F877A, asm);
    match &lines[0] {
        Line::Insn(i) => {
            assert_eq!(i.file_addr, Some(0x20));
            assert!(i.reads_file && i.writes_file && !i.writes_w);
        }
        other => panic!("expected an Insn, got {other:?}"),
    }
}

#[test]
fn dest_w_form_writes_w_not_the_file_register() {
    let asm = "    RLF 0x20, W\n";
    let lines = classify(&PIC16F877A, asm);
    match &lines[0] {
        Line::Insn(i) => {
            assert_eq!(i.file_addr, Some(0x20));
            assert!(i.reads_file && !i.writes_file && i.writes_w);
        }
        other => panic!("expected an Insn, got {other:?}"),
    }
}
