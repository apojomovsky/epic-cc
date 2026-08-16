use banking::assign_banks;

#[test]
fn passes_bank0_asm_through() {
    let asm = "    MOVF 0x20, W\n    MOVWF 0x21\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
#[should_panic]
fn rejects_bank_operand() {
    assign_banks("    MOVF 0x80, W\n");
}

#[test]
fn org_directives_pass_through_unrewritten() {
    // `.org`/`.align`/`end` take addresses/literals, not file-register
    // operands: an M11 page pad (`.org 0x0800`) or a pinned table-section
    // start (`.org 0x00D2`) must pass through untouched, never BANKSEL-
    // rewritten (relocating the program) nor range-rejected.
    let asm = "    org 0x0000\n    org 0x0800\n    org 0x00D2\n    .align 256\n    end\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
fn literal_immediates_may_exceed_0x7f() {
    // ADDLW/MOVLW/... take an 8-bit literal, not a file-register address:
    // 0xFC (= -4) is legal in a literal slot and must not be range-checked.
    let asm = "    MOVLW 0xFF\n    ADDLW 0xFC\n    MOVWF 0x21\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
fn inserts_banksel_when_bank_changes() {
    // Bank 1 then bank 0: BSF RP0 before the 0xA0 operand (rewritten to 0x20),
    // then BCF RP0 before returning to a bank-0 operand.
    let asm = "    MOVF 0xA0, W\n    MOVF 0x20, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    BCF STATUS, 5\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn common_and_sfr_operands_need_no_banksel() {
    // 0x70 (common) and STATUS (SFR) need no BANKSEL; the following bank-0
    // operand stays in the tracked bank 0, so nothing is inserted.
    let asm = "    MOVF 0x70, W\n    BTFSC STATUS, 2\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
fn no_redundant_banksel_within_same_bank() {
    // Two bank-1 operands in a row share one BANKSEL.
    let asm = "    MOVF 0xA0, W\n    MOVWF 0xE5\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    MOVWF 0x65\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn banks_2_and_3_emit_rp1_and_rewrite() {
    // Bank 2 (0x125): RP1 only. Bank 3 (0x1A5): RP0 additionally, RP1 kept.
    // 0x190-0x19F is unimplemented RAM (not bank-3 GPR), so the bank-3 test
    // address starts at 0x1A0.
    let asm = "    MOVF 0x125, W\n    MOVF 0x1A5, W\n";
    let expected = "    BSF STATUS, 6\n    MOVF 0x25, W\n    BSF STATUS, 5\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn branch_targets_reset_bank_each_arm_gets_full_banksel() {
    // A label is a branch target: the runtime bank can arrive from any arm,
    // so the linear predecessor's bank is not reliable. Each arm's first
    // banked operand must re-establish BOTH RP bits with a full BANKSEL —
    // never a partial diff against the fall-through bank.
    let asm = "    MOVF 0xA0, W\narm1:\n    MOVF 0xA5, W\narm2:\n    MOVF 0x125, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\narm1:\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\narm2:\n    BCF STATUS, 5\n    BSF STATUS, 6\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn tracking_resumes_after_label_full_banksel() {
    // After the label-triggered full BANKSEL the bank is known again:
    // same-bank operands on that arm share it (no redundant BANKSEL), and a
    // later bank change emits only the differing bit.
    let asm = "    MOVF 0xA0, W\nL:\n    MOVF 0xA5, W\n    MOVWF 0xE5\n    MOVF 0x20, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\nL:\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\n    MOVWF 0x65\n    BCF STATUS, 5\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn tracks_encountered_banksel_instructions() {
    // A hand-written BANKSEL to bank 1 means the following bank-1 operand
    // needs no new BANKSEL; the bank-0 operand that follows needs BCF RP0.
    let asm = "    BSF STATUS, 5\n    MOVF 0xA5, W\n    MOVF 0x20, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x25, W\n    BCF STATUS, 5\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
#[should_panic]
fn rejects_unbanked_sfr_operand() {
    // 0xF0-0xFF is the SFR range of bank 1; it must never be emitted as a
    // GPR operand and panics loudly.
    assign_banks("    MOVF 0xF0, W\n");
}

#[test]
fn banks_bcf_on_banked_gpr() {
    // A BCF on a banked GPR is NOT a STATUS-bank op: it must get the same
    // BANKSEL + rewrite treatment as any other file-register operand (this
    // regressed when the BANKSEL-recognition branch consumed the tokens before
    // the STATUS check, silently emitting the line verbatim).
    let asm = "    BCF 0xA0, 7\n";
    let expected = "    BSF STATUS, 5\n    BCF 0x20, 7\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn banks_bsf_on_banked_gpr() {
    // Bank 2 from bank 0: RP1 only, then the operand rewritten to 0x20.
    let asm = "    BSF 0x120, 0\n";
    let expected = "    BSF STATUS, 6\n    BSF 0x20, 0\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn status_banksel_and_gpr_bit_op_in_sequence() {
    // A genuine STATUS-bank op still updates the tracked bank, and a following
    // bit op on a same-bank GPR needs no new BANKSEL.
    let asm = "    BSF STATUS, 5\n    BSF 0xA0, 7\n";
    let expected = "    BSF STATUS, 5\n    BSF 0x20, 7\n";
    assert_eq!(assign_banks(asm), expected);
}
