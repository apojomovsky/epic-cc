use pic14_sim::{parse_hex_pic18, Pic18};

#[test]
fn a_single_nop_advances_pc_by_two_bytes_and_halts() {
    let mut p = Pic18::new(vec![0x0000]); // one NOP word
    p.run(10);
    assert_eq!(p.pc(), 2); // PC is a BYTE address on PIC18
    assert!(p.halted());
}

#[test]
fn ram_is_directly_readable_and_writable() {
    let mut p = Pic18::new(vec![0x0000]);
    p.ram_mut()[0x55] = 0x42;
    assert_eq!(p.ram()[0x55], 0x42);
}

#[test]
fn parse_hex_pic18_decodes_intel_hex_into_words() {
    let hex = asm::to_hex(&[0x55AA]);
    let words = parse_hex_pic18(&hex);
    assert_eq!(words[0], 0x55AA);
}

#[test]
fn addwf_adds_and_sets_flags() {
    // MOVLW 0x7F; MOVWF 0x20,A; ADDWF 0x20,F,A -- 0x7F + 0x7F = 0xFE,
    // no carry, no digit-carry, N set (bit7), Z clear, OV set (signed
    // 127+127 overflows into negative).
    let words = vec![0x0E7F, 0x6E20, 0x2620];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xFE);
    let status = p.ram()[0xFD8]; // STATUS is access-bank SFR: f=0xD8, a=0 -> 0xF00+0xD8=0xFD8
    assert_eq!(status & 0x01, 0, "C clear");
    assert_eq!(status & 0x08, 0x08, "OV set");
    assert_eq!(status & 0x10, 0x10, "N set");
    assert_eq!(status & 0x04, 0, "Z clear");
}

#[test]
fn subwf_computes_f_minus_w_with_no_borrow_convention() {
    // MOVLW 0x01; MOVWF 0x20,A; MOVLW 0x03; SUBWF 0x20,F,A -> 0x01 - 0x03
    // wraps to 0xFE with C=0 (borrow occurred, PIC "no borrow" convention).
    // SUBWF's d=F bit is bit9 (0x200): base 0x5C00 | 0x200 | f=0x20 = 0x5E20.
    let words = vec![0x0E01, 0x6E20, 0x0E03, 0x5E20];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xFE);
    assert_eq!(p.ram()[0xFD8] & 0x01, 0, "C clear: a borrow occurred");
}

#[test]
fn decfsz_skips_the_next_instruction_when_it_reaches_zero() {
    // MOVLW 1; MOVWF 0x20,A; DECFSZ 0x20,F,A; GOTO fail; NOP(ok)
    let words = vec![0x0E01, 0x6E20, 0x2E20, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0); // decremented to 0
                                  // The skip lands on the trailing NOP at word 5 = byte 10 (correctly
                                  // skipping the whole 2-word GOTO, not landing mid-GOTO on its second
                                  // word); `run` then executes that NOP too, ending at byte 12. Reaching
                                  // 12 cleanly (no decode panic on a stray 0xF000 continuation word, no
                                  // jump to `fail`'s bogus target) is the proof the skip worked.
    assert_eq!(p.pc(), 12);
}

#[test]
fn swapf_swaps_nibbles() {
    // SWAPF's d=F bit is bit9 (0x200): base 0x3800 | 0x200 | f=0x20 = 0x3A20.
    let words = vec![0x0EAB, 0x6E20, 0x3A20]; // MOVLW 0xAB; MOVWF 0x20,A; SWAPF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xBA);
}

#[test]
fn clrf_zeroes_and_sets_z() {
    let words = vec![0x0E42, 0x6E20, 0x6A20]; // MOVLW 0x42; MOVWF 0x20,A; CLRF 0x20,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0);
    assert_eq!(p.ram()[0xFD8] & 0x04, 0x04, "Z set");
}

#[test]
fn cpfseq_skips_when_equal() {
    // MOVLW 5; MOVWF 0x20,A; MOVLW 5; CPFSEQ 0x20,A; GOTO fail; NOP(ok)
    let words = vec![0x0E05, 0x6E20, 0x0E05, 0x6220, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    // Skip lands on the trailing NOP at word 6 = byte 12; `run` executes it
    // too, ending at byte 14 (see decfsz's test for why this is the right
    // way to prove the skip landed correctly).
    assert_eq!(p.pc(), 14);
}

#[test]
fn addwfc_adds_with_incoming_carry() {
    // MOVLW 0xFF; MOVWF 0x20,A; MOVLW 1; ADDWF 0x20,F,A (0xFF+1=0, C=1);
    // MOVLW 1; ADDWFC 0x20,F,A (0+1+1=2).
    let words = vec![0x0EFF, 0x6E20, 0x0E01, 0x2620, 0x0E01, 0x2220];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 2);
}

#[test]
fn andwf_computes_bitwise_and() {
    let words = vec![0x0EFF, 0x6E20, 0x0E0F, 0x1620]; // MOVLW 0xFF;MOVWF 0x20,A;MOVLW 0x0F;ANDWF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0x0F);
}

#[test]
fn comf_complements_the_byte() {
    let words = vec![0x0E0F, 0x6E20, 0x1E20]; // MOVLW 0x0F; MOVWF 0x20,A; COMF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xF0);
}

#[test]
fn dcfsnz_skips_when_result_is_not_zero() {
    // MOVLW 2; MOVWF 0x20,A; DCFSNZ 0x20,F,A (2-1=1, not zero, skip);
    // GOTO fail; NOP(ok)
    let words = vec![0x0E02, 0x6E20, 0x4E20, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 1);
    assert_eq!(p.pc(), 12); // landed on NOP (byte 10) then ran it
}

#[test]
fn incf_increments_and_sets_flags() {
    let words = vec![0x0EFF, 0x6E20, 0x2A20]; // MOVLW 0xFF; MOVWF 0x20,A; INCF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0);
    assert_eq!(p.ram()[0xFD8] & 0x01, 0x01, "C set: 0xFF+1 wraps");
    assert_eq!(p.ram()[0xFD8] & 0x04, 0x04, "Z set");
}

#[test]
fn incfsz_skips_when_it_wraps_to_zero() {
    let words = vec![0x0EFF, 0x6E20, 0x3E20, 0xEF10, 0xF000, 0x0000]; // ...;INCFSZ 0x20,F,A;GOTO fail;NOP
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0);
    assert_eq!(p.pc(), 12);
}

#[test]
fn infsnz_skips_when_result_is_not_zero() {
    let words = vec![0x0E01, 0x6E20, 0x4A20, 0xEF10, 0xF000, 0x0000]; // ...;INFSNZ 0x20,F,A;GOTO fail;NOP
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 2);
    assert_eq!(p.pc(), 12);
}

#[test]
fn iorwf_computes_bitwise_or() {
    let words = vec![0x0E0F, 0x6E20, 0x0EF0, 0x1220]; // MOVLW 0x0F;MOVWF 0x20,A;MOVLW 0xF0;IORWF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xFF);
}

#[test]
fn movf_copies_f_and_sets_flags() {
    let words = vec![0x0E00, 0x6E20, 0x5220]; // MOVLW 0; MOVWF 0x20,A; MOVF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0xFD8] & 0x04, 0x04, "Z set");
}

#[test]
fn rlcf_rotates_left_through_carry() {
    // MOVLW 0x80; MOVWF 0x20,A -> ram=0x80, C starts clear.
    // RLCF 0x20,F,A: result = (0x80<<1)|0 = 0, C becomes 1 (old bit7).
    let words = vec![0x0E80, 0x6E20, 0x3620];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0);
    assert_eq!(
        p.ram()[0xFD8] & 0x01,
        0x01,
        "C set from the rotated-out bit7"
    );
}

#[test]
fn rlncf_rotates_left_without_carry() {
    let words = vec![0x0E80, 0x6E20, 0x4620]; // MOVLW 0x80; MOVWF 0x20,A; RLNCF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 1, "bit7 wraps to bit0");
}

#[test]
fn rrcf_rotates_right_through_carry() {
    let words = vec![0x0E01, 0x6E20, 0x3220]; // MOVLW 1; MOVWF 0x20,A; RRCF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0);
    assert_eq!(
        p.ram()[0xFD8] & 0x01,
        0x01,
        "C set from the rotated-out bit0"
    );
}

#[test]
fn rrncf_rotates_right_without_carry() {
    let words = vec![0x0E01, 0x6E20, 0x4220]; // MOVLW 1; MOVWF 0x20,A; RRNCF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0x80, "bit0 wraps to bit7");
}

#[test]
fn subfwb_subtracts_with_borrow() {
    // MOVLW 5; MOVWF 0x20,A -> ram=5. MOVLW 2. SUBFWB 0x20,F,A with C
    // initially clear (borrow-in=1): 5 - 2 - 1 = 2.
    let words = vec![0x0E05, 0x6E20, 0x0E02, 0x5620];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 2);
}

#[test]
fn xorwf_computes_bitwise_xor() {
    let words = vec![0x0EFF, 0x6E20, 0x0E0F, 0x1A20]; // MOVLW 0xFF;MOVWF 0x20,A;MOVLW 0x0F;XORWF 0x20,F,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xF0);
}

#[test]
fn cpfsgt_skips_when_f_greater_than_w() {
    // MOVLW 5;MOVWF 0x20,A;MOVLW 3;CPFSGT 0x20,A (5>3, skip);GOTO fail;NOP
    let words = vec![0x0E05, 0x6E20, 0x0E03, 0x6420, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.pc(), 14);
}

#[test]
fn cpfslt_skips_when_f_less_than_w() {
    let words = vec![0x0E03, 0x6E20, 0x0E05, 0x6020, 0xEF10, 0xF000, 0x0000]; // f=3 < W=5
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.pc(), 14);
}

#[test]
fn negf_negates_in_place() {
    let words = vec![0x0E05, 0x6E20, 0x6C20]; // MOVLW 5; MOVWF 0x20,A; NEGF 0x20,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xFB); // -5 as u8
}

#[test]
fn setf_sets_all_bits() {
    let words = vec![0x0E00, 0x6E20, 0x6820]; // MOVLW 0; MOVWF 0x20,A; SETF 0x20,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xFF);
}

#[test]
fn tstfsz_skips_when_f_is_zero() {
    let words = vec![0x0E00, 0x6E20, 0x6620, 0xEF10, 0xF000, 0x0000]; // ...;TSTFSZ 0x20,A;GOTO fail;NOP
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.pc(), 12);
}

#[test]
fn bsf_sets_a_bit_without_touching_others() {
    let words = vec![0x0E0F, 0x6E20, 0x8A20]; // MOVLW 0x0F; MOVWF 0x20,A; BSF 0x20,5,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0x2F);
}

#[test]
fn bcf_clears_a_bit_without_touching_others() {
    let words = vec![0x0EFF, 0x6E20, 0x9820]; // MOVLW 0xFF; MOVWF 0x20,A; BCF 0x20,4,A
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x20], 0xEF);
}

#[test]
fn btfsc_skips_when_the_bit_is_clear() {
    // MOVLW 0; MOVWF 0x20,A; BTFSC 0x20,0,A; GOTO fail; NOP
    let words = vec![0x0E00, 0x6E20, 0xB020, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.pc(), 12);
}

#[test]
fn btfss_skips_when_the_bit_is_set() {
    // MOVLW 1; MOVWF 0x20,A; BTFSS 0x20,0,A; GOTO fail; NOP
    let words = vec![0x0E01, 0x6E20, 0xA020, 0xEF10, 0xF000, 0x0000];
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.pc(), 12);
}

#[test]
fn btg_toggles_a_bit() {
    let words = vec![0x0E00, 0x6E20, 0x7820, 0x7820]; // MOVLW 0; MOVWF 0x20,A; BTG 0x20,4,A twice
    let mut p = Pic18::new(words);
    p.run(3);
    assert_eq!(p.ram()[0x20], 0x10);
    p.run(1);
    assert_eq!(p.ram()[0x20], 0x00);
}

#[test]
fn movlw_loads_w_with_no_flags() {
    let mut p = Pic18::new(vec![0x0E42]);
    p.run(1);
    assert_eq!(p.w(), 0x42);
}

#[test]
fn addlw_adds_to_w_with_flags() {
    let words = vec![0x0E7F, 0x0F01]; // MOVLW 0x7F; ADDLW 0x01
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0x80);
    assert_eq!(
        p.ram()[0xFD8] & 0x08,
        0x08,
        "OV set: 127+1 signed-overflows"
    );
}

#[test]
fn sublw_computes_k_minus_w() {
    let words = vec![0x0E03, 0x0801]; // MOVLW 3; SUBLW 1 -> 1 - 3 = 0xFE, C clear (borrow)
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0xFE);
    assert_eq!(p.ram()[0xFD8] & 0x01, 0);
}

#[test]
fn iorlw_ors_into_w() {
    let words = vec![0x0E0F, 0x09F0]; // MOVLW 0x0F; IORLW 0xF0
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0xFF);
}

#[test]
fn xorlw_xors_into_w() {
    let words = vec![0x0EFF, 0x0A0F]; // MOVLW 0xFF; XORLW 0x0F
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0xF0);
}

#[test]
fn andlw_ands_into_w() {
    let words = vec![0x0EFF, 0x0B0F]; // MOVLW 0xFF; ANDLW 0x0F
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0x0F);
}

#[test]
fn call_pushes_return_address_and_goto_jumps_unconditionally() {
    let src = "    CALL sub\n    NOP\n    GOTO fin\nsub:\n    RETURN\nfin:\n    NOP\n";
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(20);
    assert!(p.halted());
}

#[test]
fn rcall_and_bra_and_conditional_branches_execute() {
    let src = "here:\n    MOVLW 0\n    BTFSC 0xFD8,2,A\n    BRA here\n    RCALL sub\n    BRA fin\nsub:\n    RETURN\nfin:\n    NOP\n";
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(20);
    assert!(p.halted());
}

#[test]
fn retlw_returns_and_loads_w() {
    // `sub:` sits right after the main flow with no separating halt, so
    // this only runs exactly the two real steps (CALL, then RETLW) rather
    // than relying on `halted()` — falling through past RETLW would just
    // re-enter `sub` with an empty stack, which isn't what this test is
    // about.
    let src = "    CALL sub\n    NOP\nsub:\n    RETLW 0x42\n";
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0x42);
}

#[test]
fn stkptr_and_tos_registers_reflect_the_call_stack() {
    let src = "    CALL sub\n    NOP\nsub:\n    NOP\n";
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(1); // execute the CALL only
    assert_eq!(p.ram()[0xFFC], 1, "STKPTR == 1 after one CALL");
    // CALL is a 2-word (4-byte) instruction, so the return address pushed
    // is CALL's own address + 4, not +2. TOSL/TOSH/TOSU hold it split into
    // bytes.
    assert_eq!(p.ram()[0xFFD], 4);
    assert_eq!(p.ram()[0xFFE], 0);
    assert_eq!(p.ram()[0xFFF], 0);
}

#[test]
fn indf0_reads_and_writes_through_fsr0() {
    let src = "    LFSR 0, 0x55\n    MOVLW 0x42\n    MOVWF 0xFEF,A\n"; // MOVWF INDF0
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x55], 0x42);
}

#[test]
fn postinc0_writes_then_advances_fsr0() {
    let src = "    LFSR 0, 0x55\n    MOVLW 0x42\n    MOVWF 0xFEE,A\n"; // MOVWF POSTINC0
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x55], 0x42);
    assert_eq!(p.ram()[0xFE9], 0x56, "FSR0L advanced to 0x56");
}

#[test]
fn preinc0_advances_then_writes() {
    let src = "    LFSR 0, 0x55\n    MOVLW 0x42\n    MOVWF 0xFEC,A\n"; // MOVWF PREINC0
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x56], 0x42);
    assert_eq!(p.ram()[0x55], 0);
}

#[test]
fn postdec0_writes_then_decrements() {
    let src = "    LFSR 0, 0x55\n    MOVLW 0x42\n    MOVWF 0xFED,A\n"; // MOVWF POSTDEC0
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x55], 0x42);
    assert_eq!(p.ram()[0xFE9], 0x54);
}

#[test]
fn plusw0_reads_fsr0_plus_signed_w_without_side_effect() {
    let src =
        "    LFSR 0, 0x55\n    MOVLW 0x42\n    MOVWF 0x56,A\n    MOVLW 1\n    MOVF 0xFEB,W,A\n"; // MOVF PLUSW0,W
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.w(), 0x42, "read ram[0x55+1]=ram[0x56]");
    assert_eq!(p.ram()[0xFE9], 0x55, "FSR0L unchanged by PLUSW0");
}

#[test]
fn mulwf_produces_an_unsigned_16_bit_product_in_prodh_prodl() {
    let src = "    MOVLW 0x10\n    MOVWF 0x20,A\n    MOVLW 0x10\n    MULWF 0x20,A\n"; // 0x10*0x10=0x100
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0xFF3], 0x00, "PRODL");
    assert_eq!(p.ram()[0xFF4], 0x01, "PRODH");
}

#[test]
fn mullw_produces_an_unsigned_16_bit_product_in_prodh_prodl() {
    let src = "    MOVLW 0x20\n    MULLW 0x03\n"; // 0x20*0x03=0x60
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0xFF3], 0x60, "PRODL");
    assert_eq!(p.ram()[0xFF4], 0x00, "PRODH");
}

#[test]
fn movlb_selects_the_bank_for_a_subsequent_banked_access() {
    let src = "    MOVLB 1\n    MOVLW 0x42\n    MOVWF 0x20,B\n"; // writes physical 0x120
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.run(10);
    assert_eq!(p.ram()[0x120], 0x42);
}

#[test]
fn sleep_halts_the_simulator() {
    let mut p = Pic18::new(vec![0x0003, 0x0000]); // SLEEP; NOP (never reached)
    p.run(10);
    assert!(p.halted());
    assert_eq!(p.pc(), 0, "SLEEP does not advance pc");
}

#[test]
fn clrwdt_advances_pc_with_no_other_effect() {
    let mut p = Pic18::new(vec![0x0004]); // CLRWDT
    p.run(1);
    assert_eq!(p.pc(), 2);
}

#[test]
fn push_and_pop_the_hardware_stack_without_jumping() {
    let mut p = Pic18::new(vec![0x0005, 0x0006, 0x0000]); // PUSH; POP; NOP
    p.run(1);
    assert_eq!(p.ram()[0xFFC], 1, "STKPTR after PUSH");
    p.run(1);
    assert_eq!(p.ram()[0xFFC], 0, "STKPTR after POP");
    assert_eq!(p.pc(), 4, "POP does not jump");
}

#[test]
fn daw_adjusts_w_to_valid_bcd() {
    // MOVLW 0x0B; DAW -> low nibble (0xB=11) > 9, so W += 6 = 0x11.
    let words = vec![0x0E0B, 0x0007];
    let mut p = Pic18::new(words);
    p.run(2);
    assert_eq!(p.w(), 0x11);
}

#[test]
fn reset_reinitializes_pc_and_w() {
    let words = vec![0x0E42, 0x00FF, 0x0003]; // MOVLW 0x42; RESET; SLEEP
    let mut p = Pic18::new(words);
    p.run(1);
    assert_eq!(p.w(), 0x42);
    p.run(1);
    assert_eq!(p.w(), 0, "RESET clears W");
    assert_eq!(p.pc(), 0, "RESET jumps to the reset vector");
}

#[test]
fn movff_dereferences_indf_and_postinc_operands() {
    // FSR0 -> 0x020 (a GPR byte holding 0x42, poked in directly); FSR1 ->
    // 0x021 (initially 0x00). `MOVFF INDF0, POSTINC1` (operands 0xFEF,
    // 0xFE6) must copy the byte FSR0 points to (0x42) into the byte FSR1
    // points to, then increment FSR1 -- not literally treat the SFR
    // addresses 0xFEF/0xFE6 as if they were ordinary RAM locations.
    let src = "    LFSR 0, 0x020\n    LFSR 1, 0x021\n    MOVFF 0xFEF, 0xFE6\n";
    let words = asm::assemble_pic18(src);
    let mut p = Pic18::new(words);
    p.ram_mut()[0x020] = 0x42;
    p.run(10);
    assert_eq!(
        p.ram()[0x021],
        0x42,
        "the byte at 0x021 must be overwritten via POSTINC1's dereference of FSR1"
    );
    // FSR1L lives at 0xF00 + 0xE1 = 0xFE1 (resolve_f's FSR1 match arm:
    // fsrn_lo = 0xE1). FSR1 must have advanced past 0x021 after the
    // post-increment.
    assert_eq!(
        p.ram()[0xFE1],
        0x22,
        "FSR1L must read 0x22 after POSTINC1's increment"
    );
}
