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
fn call_resets_tracked_bank_next_operand_gets_full_banksel() {
    // A CALL is a runtime boundary just like a label: the callee's body (its
    // own BANKSELs and banked operands) can leave the RP bits in any state,
    // and the callee's text is not visible in the caller's. The tracked bank
    // must not cross a CALL — a caller's next banked operand (even one in
    // the SAME bank as before the CALL) re-establishes BOTH RP bits with a
    // full BANKSEL, never a partial diff against the pre-CALL bank.
    let asm = "    MOVF 0xA0, W\n    CALL f\n    MOVF 0xA5, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    CALL f\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn tracking_resumes_after_call_full_banksel() {
    // After the CALL-triggered full BANKSEL the bank is known again:
    // same-bank operands on that straight-line stretch share it (no
    // redundant BANKSEL), and a later bank change emits only the differing
    // bit — exactly like the label-reset behavior.
    let asm = "    MOVF 0xA0, W\n    CALL f\n    MOVF 0xA5, W\n    MOVWF 0xE5\n    MOVF 0x20, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    CALL f\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\n    MOVWF 0x65\n    BCF STATUS, 5\n    MOVF 0x20, W\n";
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

#[test]
fn bank0_only_program_skips_all_banksels() {
    // Issue #16 (left over from #13): a program that provably never leaves
    // bank 0 — no banked GPR operand (0xA0-0xEF / 0x120-0x16F / 0x1A0-0x1EF)
    // and no hand-written `BCF/BSF STATUS, 5/6` — runs entirely in bank 0,
    // so every label/CALL reset can be skipped instead of emitting the dead
    // full BANKSEL (`BCF STATUS, 5` + `BCF STATUS, 6`) preamble. The reset
    // is only ever reached via the reset vector (bank 0) or a fall-through
    // that never changed the bank, and nothing in the text can change it.
    let asm = "    MOVF 0x20, W\nL:\n    MOVF 0x21, W\n    CALL f\n    MOVF 0x22, W\n    MOVWF 0x23\nf:\n    MOVF 0x24, W\n    RETURN\n";
    let expected = "    MOVF 0x20, W\nL:\n    MOVF 0x21, W\n    CALL f\n    MOVF 0x22, W\n    MOVWF 0x23\nf:\n    MOVF 0x24, W\n    RETURN\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn bank0_only_with_handwritten_bank_op_keeps_resets() {
    // A hand-written `BCF/BSF STATUS, 5/6` means the program touches the
    // bank bits itself, so the pass cannot prove the bank stays 0 at every
    // label/CALL: the resets stay (each arm's first banked operand gets the
    // full BANKSEL, exactly as before issue #16).
    let asm = "    BSF STATUS, 5\n    BCF STATUS, 5\nL:\n    MOVF 0x20, W\n";
    let expected = "    BSF STATUS, 5\n    BCF STATUS, 5\nL:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn bank0_only_with_movwf_status_keeps_resets() {
    // `MOVWF STATUS` writes all of STATUS from W, including the RP bits:
    // after it the tracked bank is unknowable, so the pass must fall back to
    // the label/CALL resets (an ISR's restore ends with `MOVWF STATUS`, so
    // every interrupt program stays on this path and its layout is
    // unchanged).
    let asm = "    MOVWF STATUS\nL:\n    MOVF 0x20, W\n";
    let expected = "    MOVWF STATUS\nL:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

// ---------------------------------------------------------------------------
// Issue #13: reclaim redundant BANKSEL sequences
// ---------------------------------------------------------------------------

// ---- Item 2: the CALL reset is redundant when the callee provably exits
// ---- with a fixed bank (the caller can keep tracking that bank).

#[test]
fn call_to_bank0_callee_skips_redundant_banksel() {
    // The caller is in bank 1, calls a bank-0-only callee (its body has no
    // banked operand, so it provably exits bank 0), then touches a bank-0
    // operand. The callee's exit bank is provable, so the full BANKSEL after
    // the CALL is redundant — the tracked bank is 0 already.
    let asm = "    MOVF 0xA0, W\n    CALL helper\n    MOVF 0x20, W\nhelper:\n    MOVF 0x21, W\n    RETURN\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    CALL helper\n    MOVF 0x20, W\nhelper:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x21, W\n    RETURN\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn call_to_bank1_callee_keeps_bank() {
    // The callee's last banked operand is bank 1, so it provably exits bank
    // 1; the caller's next operand is bank 1 too — no BANKSEL after the
    // CALL (before the fix, every CALL reset the tracked bank and the next
    // banked operand got a full BANKSEL).
    let asm = "    MOVF 0xA0, W\n    CALL helper\n    MOVF 0xA5, W\nhelper:\n    MOVF 0xA1, W\n    RETURN\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    CALL helper\n    MOVF 0x25, W\nhelper:\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x21, W\n    RETURN\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn call_exit_bank_is_transitive() {
    // outer calls inner (which exits bank 0), so outer provably exits bank
    // 0 too — the caller's next bank-0 operand needs no BANKSEL after
    // `CALL outer`.
    let asm = "    MOVF 0xA0, W\n    CALL outer\n    MOVF 0x20, W\nouter:\n    CALL inner\n    RETURN\ninner:\n    MOVF 0x21, W\n    RETURN\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    CALL outer\n    MOVF 0x20, W\nouter:\n    CALL inner\n    RETURN\ninner:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x21, W\n    RETURN\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn call_to_unknown_bank_callee_keeps_full_reset() {
    // A callee whose exit bank is not provable keeps the full BANKSEL after
    // the CALL — the optimization must never skip a needed reset. The
    // callee's only banked operand sits under a BTFSC: if the skip is taken
    // the bank stays 0 (the caller's bank), if not it becomes 1 — the two
    // paths diverge, so the exit bank is not provable.
    let asm = "    MOVF 0x20, W\n    CALL helper\n    MOVF 0xA5, W\nhelper:\n    BTFSC STATUS, 2\n    MOVF 0xA1, W\n    RETURN\n";
    let expected = "    MOVF 0x20, W\n    CALL helper\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\nhelper:\n    BTFSC STATUS, 2\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x21, W\n    RETURN\n";
    assert_eq!(assign_banks(asm), expected);
}

// ---- Item 3: bank-select ops in bit-number forms are recognized.

#[test]
fn tracks_banksel_with_attached_comma() {
    // `BCF STATUS,5` (the comma attached to the register token) is a
    // STATUS bank op: it must update the tracked bank, so the following
    // bank-1 operand gets its own BANKSEL (before the fix the op was not
    // recognized and the bank-1 operand silently ran with the bank the
    // hand-written BCF left).
    let asm = "    MOVF 0xA0, W\n    BCF STATUS,5\n    MOVF 0xA5, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    BCF STATUS,5\n    BSF STATUS, 5\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn tracks_banksel_by_status_address() {
    // `BCF 0x03, 5` — STATUS by its register address — is a STATUS bank
    // op too (0x03 IS STATUS on the PIC16F877A).
    let asm = "    MOVF 0xA0, W\n    BCF 0x03, 5\n    MOVF 0xA5, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    BCF 0x03, 5\n    BSF STATUS, 5\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn movwf_status_resets_tracked_bank() {
    // `MOVWF STATUS` writes all of STATUS from W, including the RP bits:
    // the tracked bank becomes unknowable, so the next banked operand gets
    // a FULL BANKSEL (before the fix the pass kept tracking the pre-write
    // bank and emitted only a partial diff).
    let asm = "    MOVF 0xA0, W\n    MOVWF STATUS\n    MOVF 0xA5, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    MOVWF STATUS\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn movwf_status_by_address_resets_tracked_bank() {
    // The address form `MOVWF 0x03` is the same STATUS write.
    let asm = "    MOVF 0xA0, W\n    MOVWF 0x03\n    MOVF 0xA5, W\n";
    let expected = "    BSF STATUS, 5\n    MOVF 0x20, W\n    MOVWF 0x03\n    BSF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x25, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn bank0_only_with_attached_comma_bank_op_keeps_resets() {
    // The bit-number forms disqualify a program from the bank-0-only skip
    // (they touch the bank bits, so the pass cannot prove the bank stays 0
    // at every label/CALL).
    let asm = "    BCF STATUS,5\nL:\n    MOVF 0x20, W\n";
    let expected = "    BCF STATUS,5\nL:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn bank0_only_with_status_address_bank_op_keeps_resets() {
    let asm = "    BCF 0x03, 5\nL:\n    MOVF 0x20, W\n";
    let expected = "    BCF 0x03, 5\nL:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}

#[test]
fn bank0_only_with_movwf_status_address_keeps_resets() {
    let asm = "    MOVWF 0x03\nL:\n    MOVF 0x20, W\n";
    let expected = "    MOVWF 0x03\nL:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x20, W\n";
    assert_eq!(assign_banks(asm), expected);
}
