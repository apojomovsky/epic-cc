use peephole::optimize;

#[test]
fn passes_through() {
    let asm = "    NOP\n";
    assert_eq!(optimize(asm), asm);
}

#[test]
fn same_page_call_elides_trailing_restore() {
    // The tracked PCLATH literal (0x08) is unchanged by the CALL, so the
    // restore pair writing the same literal is redundant and dropped.
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    CALL f\n    MOVLW 0x08\n    MOVWF PCLATH\n";
    let expected = "    MOVLW 0x08\n    MOVWF PCLATH\n    CALL f\n";
    assert_eq!(optimize(asm), expected);
}

#[test]
fn cross_page_call_keeps_both_sets() {
    // The restore literal (0x00) differs from the pre-set (0x08), so neither
    // pair is redundant.
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    CALL f\n    MOVLW 0x00\n    MOVWF PCLATH\n";
    assert_eq!(optimize(asm), asm);
}

#[test]
fn readers_window_set_elided_when_equal_to_tracked() {
    // A reader's set with HIGH(t) == 0x08 after a `MOVLW 0x08; MOVWF PCLATH`
    // writes the same window the tracking already holds -> elided.
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    MOVLW 0x08\n    MOVWF PCLATH\n";
    let expected = "    MOVLW 0x08\n    MOVWF PCLATH\n";
    assert_eq!(optimize(asm), expected);
}

#[test]
fn readers_window_set_kept_when_different_from_tracked() {
    // A reader needing window HIGH(t) == 0x09 after a tracked 0x08 must keep
    // its set (the window differs).
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    MOVLW 0x09\n    MOVWF PCLATH\n";
    assert_eq!(optimize(asm), asm);
}

#[test]
fn tracked_value_persists_across_goto_and_label() {
    // GOTO and a label do not write PCLATH, so the tracked literal survives
    // them and the second equal pair is dropped.
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    GOTO lbl\nlbl:\n    MOVLW 0x08\n    MOVWF PCLATH\n";
    let expected = "    MOVLW 0x08\n    MOVWF PCLATH\n    GOTO lbl\nlbl:\n";
    assert_eq!(optimize(asm), expected);
}

#[test]
fn non_pclath_lines_are_unchanged() {
    // Lines that do not form a PCLATH pair pass through verbatim.
    let asm = "    NOP\n    MOVLW 0x05\n    MOVWF 0x20\n    ADDLW LOW(t)\n    MOVWF PCL\n    NOP\n";
    assert_eq!(optimize(asm), asm);
}

#[test]
fn identical_symbolic_operands_elide() {
    // The real pipeline emits `MOVLW PAGE(main)`; an identical operand writes
    // the same resolved literal, so the duplicate pair is dropped.
    let asm = "    MOVLW PAGE(main)\n    MOVWF PCLATH\n    CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n";
    let expected = "    MOVLW PAGE(main)\n    MOVWF PCLATH\n    CALL helper\n";
    assert_eq!(optimize(asm), expected);
}

#[test]
fn different_symbolic_operands_kept() {
    // `PAGE(helper)` (target) vs `PAGE(main)` (restore) differ -> both kept
    // (conservative: we cannot prove they resolve to the same literal).
    let asm = "    MOVLW PAGE(helper)\n    MOVWF PCLATH\n    CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n";
    assert_eq!(optimize(asm), asm);
}
