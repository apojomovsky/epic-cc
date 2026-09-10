//! PIC baseline (12-bit-word core) simulator tests, one per instruction
//! group from the P1 acceptance (docs/37 section 3): the byte ops (flags,
//! d routing), bit ops, literals, the 2-level CALL/RETLW stack, GOTO page
//! bit, PCL write, direct bank selection via `BSF`/`BCF FSR,5`, indirect
//! addressing through INDF, shared GPR mirroring, TRIS/OPTION shadow
//! writes, and SLEEP halt.

use asm::assemble_pic_baseline;
use pic14_sim::{parse_hex, PicBaseline};

/// Assemble `body` (org-anchored) with the core register symbols the
/// source text needs, then run it to completion on the 509.
fn run_asm(body: &str) -> PicBaseline {
    let src = format!(
        "INDF   equ 0x000\n\
         PCL    equ 0x002\n\
         STATUS equ 0x003\n\
         FSR    equ 0x004\n\
         {body}"
    );
    let words = assemble_pic_baseline(&src);
    let mut p = PicBaseline::with_device(&device::PIC12F509, words);
    p.run(10_000);
    p
}

#[test]
fn addwf_adds_w_and_f_setting_carry_dc_and_z() {
    // 0x0F + 0x01 = 0x10: C = 0, DC = 1, Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x0F\n\
             MOVWF 0x10\n\
             MOVLW 0x01\n\
             ADDWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x10);
    assert_eq!(p.ram()[0x03] & 0b001, 0, "C = 0");
    assert_eq!(p.ram()[0x03] & 0b010, 0b010, "DC = 1");
    assert_eq!(p.ram()[0x03] & 0b100, 0, "Z = 0");
    // 0xFF + 0x01 = 0x00: C = 1, Z = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xFF\n\
             MOVWF 0x10\n\
             MOVLW 0x01\n\
             ADDWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = 1");
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
}

#[test]
fn subwf_subtracts_w_from_f_setting_borrow() {
    // 0x10 - 0x01 = 0x0F: C = 1 (no borrow), Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x10\n\
             MOVWF 0x10\n\
             MOVLW 0x01\n\
             SUBWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x0F);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = 1 (no borrow)");
    // 0x01 - 0x10 = 0xF1: C = 0 (borrow).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x10\n\
             MOVLW 0x10\n\
             SUBWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0xF1);
    assert_eq!(p.ram()[0x03] & 0b001, 0, "C = 0 (borrow)");
}

#[test]
fn logic_ops_set_z_and_route_d() {
    // ANDWF 0x0F & 0x0F = 0x0F, Z = 0, result to W (d = 0).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x0F\n\
             MOVWF 0x10\n\
             MOVLW 0x0F\n\
             ANDWF 0x10, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x0F);
    assert_eq!(p.ram()[0x10], 0x0F, "d = 0 leaves f untouched");
    // IORWF 0x00 | 0x00 = 0x00, Z = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x00\n\
             MOVWF 0x10\n\
             MOVLW 0x00\n\
             IORWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
    // XORWF 0xFF ^ 0xFF = 0x00, Z = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xFF\n\
             MOVWF 0x10\n\
             MOVLW 0xFF\n\
             XORWF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
}

#[test]
fn movf_clrf_clrw_and_comf() {
    // MOVF 0x10, W copies f to W and sets Z.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             MOVF 0x10, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x5A);
    // CLRF clears f and sets Z.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             CLRF 0x10\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
    // CLRW clears W and sets Z.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             CLRW\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x00);
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
    // COMF 0x00 -> 0xFF, Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             CLRF 0x10\n\
             COMF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0xFF);
    assert_eq!(p.ram()[0x03] & 0b100, 0, "Z = 0");
}

#[test]
fn incf_decf_and_swapf() {
    // INCF 0x0F -> 0x10, Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x0F\n\
             MOVWF 0x10\n\
             INCF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x10);
    // DECF 0x00 -> 0xFF, Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             CLRF 0x10\n\
             DECF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0xFF);
    // SWAPF 0xAB -> 0xBA, no flags.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xAB\n\
             MOVWF 0x10\n\
             SWAPF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0xBA);
}

#[test]
fn rlf_and_rrf_rotate_through_carry() {
    // RLF 0x80 with C = 0 -> 0x00, C = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x80\n\
             MOVWF 0x10\n\
             RLF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = MSb of source");
    // RRF 0x01 with C = 0 -> 0x00, C = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x10\n\
             RRF 0x10, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = LSb of source");
}

#[test]
fn decfsz_and_incfsz_skip_when_zero() {
    // DECFSZ 0x01 -> 0x00 skips the next instruction.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x10\n\
             DECFSZ 0x10, F\n\
             MOVLW 0xAA\n\
             MOVLW 0xBB\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xBB, "the MOVLW 0xAA after the skip is bypassed");
    // INCFSZ 0xFF -> 0x00 skips.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xFF\n\
             MOVWF 0x10\n\
             INCFSZ 0x10, F\n\
             MOVLW 0xAA\n\
             MOVLW 0xBB\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xBB, "the MOVLW 0xAA after the skip is bypassed");
}

#[test]
fn bit_ops_set_clear_and_test() {
    // BSF sets a bit, BCF clears it.
    let p = run_asm(
        "    org 0x20\n\
             CLRF 0x10\n\
             BSF 0x10, 3\n\
             BCF 0x10, 3\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x00);
    // BTFSC skips when the bit is clear.
    let p = run_asm(
        "    org 0x20\n\
             CLRF 0x10\n\
             BTFSC 0x10, 0\n\
             MOVLW 0xAA\n\
             MOVLW 0xBB\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xBB, "BTFSC skips when the bit is clear");
    // BTFSS skips when the bit is set.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x10\n\
             BTFSS 0x10, 0\n\
             MOVLW 0xAA\n\
             MOVLW 0xBB\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xBB, "BTFSS skips when the bit is set");
}

#[test]
fn literals_load_and_combine_with_w() {
    // MOVLW loads W.
    let p = run_asm("    org 0x20\n    MOVLW 0x55\n    SLEEP\n");
    assert_eq!(p.w(), 0x55);
    // ANDLW 0x0F & 0x0F = 0x0F, Z = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x0F\n\
             ANDLW 0x0F\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x0F);
    // IORLW 0x00 | 0xF0 = 0xF0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x00\n\
             IORLW 0xF0\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xF0);
    // XORLW 0xFF ^ 0xFF = 0x00, Z = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xFF\n\
             XORLW 0xFF\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x00);
    assert_eq!(p.ram()[0x03] & 0b100, 0b100, "Z = 1");
}

#[test]
fn call_and_retlw_use_the_two_level_stack() {
    // CALL pushes PC+1; RETLW pops it and loads W. A second CALL shifts
    // level 1 to level 2, so a nested return restores the outer address.
    let p = run_asm(
        "    org 0x20\n\
             CALL sub\n\
             SLEEP\n\
    sub:    RETLW 0x42\n",
    );
    assert_eq!(p.w(), 0x42, "RETLW loads the literal into W");
    assert_eq!(p.pc(), 0x21, "RETLW returns to the instruction after CALL");
    // Nested: CALL outer; CALL inner; RETLW -> outer; RETLW -> main. The
    // second CALL shifts the outer return to level 2, so the inner RETLW
    // restores it and the outer RETLW pops it back to main. The NOP at 0x23
    // is inner's return target, so both RETLWs actually execute.
    let p = run_asm(
        "    org 0x20\n\
             CALL outer\n\
             SLEEP\n\
    outer:  CALL inner\n\
             NOP\n\
             RETLW 0x11\n\
    inner:  RETLW 0x22\n",
    );
    assert_eq!(
        p.w(),
        0x11,
        "outer RETLW loads its literal after inner returns"
    );
    assert_eq!(p.pc(), 0x21, "outer RETLW returns to main");
}

#[test]
fn goto_uses_the_page_bit_and_call_clears_pc8() {
    // GOTO 0x1FF with PA0 = 0 -> PC = 0x1FF.
    let p = run_asm("    org 0x20\n    GOTO 0x1FF\n");
    assert_eq!(p.pc(), 0x1FF);
    // CALL 0x0FF forces PC<8> = 0 (DS41236E section 4.7).
    let p = run_asm("    org 0x20\n    CALL 0x0FF\n");
    assert_eq!(p.pc(), 0x0FF, "CALL clears PC<8>");
}

#[test]
fn pcl_write_sets_pc_from_w_and_pa0() {
    // MOVWF PCL: PC<7:0> = W, PC<8> = 0, PC<9> = PA0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x30\n\
             MOVWF PCL\n\
             SLEEP\n",
    );
    assert_eq!(p.pc(), 0x30, "PCL write jumps to W");
}

#[test]
fn direct_operands_page_by_fsr5() {
    // With FSR<5> = 0, MOVWF 0x10 lands in bank 0 (0x10); with FSR<5> = 1
    // it lands in bank 1 (0x30). The SFR block and shared GPR stay
    // bank-independent.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             BSF FSR, 5\n\
             MOVLW 0x6B\n\
             MOVWF 0x10\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x5A, "bank 0 GPR");
    assert_eq!(p.ram()[0x30], 0x6B, "bank 1 GPR");
    // Shared GPR 0x07 is the same cell from either bank.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             MOVWF 0x07\n\
             BSF FSR, 5\n\
             MOVF 0x07, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x5A, "shared GPR visible from bank 1");
}

#[test]
fn indirect_addressing_through_indf() {
    // INDF reads/writes RAM[FSR & 0x3F]: the full flat address, bank bits
    // and offset together.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             MOVLW 0x10\n\
             MOVWF FSR\n\
             MOVF INDF, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x5A, "INDF reads RAM[FSR]");
    // Writing through INDF with FSR = 0x30 lands in bank 1 GPR.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x30\n\
             MOVWF FSR\n\
             MOVLW 0x6B\n\
             MOVWF INDF\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x30], 0x6B, "INDF write through FSR = 0x30");
}

#[test]
fn tris_and_option_write_shadow_registers() {
    // TRIS f and OPTION are write-only control ops; the shadow registers
    // are not addressable in the file map.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x3F\n\
             TRIS 6\n\
             MOVLW 0x55\n\
             OPTION\n\
             SLEEP\n",
    );
    assert_eq!(p.tris(), 0x3F, "TRIS 6 writes the TRISGPIO shadow");
    assert_eq!(p.option(), 0x55, "OPTION writes the OPTION shadow");
}

#[test]
fn sleep_halts_the_core() {
    let p = run_asm("    org 0x20\n    SLEEP\n");
    assert!(p.halted());
    assert_eq!(p.pc(), 0x20, "SLEEP does not advance the PC");
}

#[test]
fn parse_hex_decodes_12_bit_words() {
    // MOVLW 0x10 -> 0x0C10 -> bytes 10 0C; MOVWF FSR -> 0x0024 -> 24 00.
    let hex = ":020000040000FA\n:04000000100C2400F0\n:00000001FF\n";
    let words = parse_hex(hex);
    assert_eq!(words[0], 0x0C10);
    assert_eq!(words[1], 0x0024);
}
