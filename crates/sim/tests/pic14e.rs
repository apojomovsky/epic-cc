//! PIC14E (Enhanced Mid-range) simulator tests, one per instruction group
//! from the P1 acceptance (docs/33 section 4): the shifts, MOVLB/MOVLP,
//! the branches (BRA/BRW/CALLW), the FSR machinery (ADDFSR + the four
//! MOVIW/MOVWI modifier forms + indexed), OPTION/TRIS, and the negative
//! STATUS test (bits 7-5 read as 0 and BCF/BSF against them is inert).
//! Plus the three FSR address regions (traditional, linear alias, program
//! flash with its extra cycle).

use asm::assemble_pic14e;
use pic14_sim::Pic14e;

/// Assemble `body` (org-anchored) with the core register symbols the
/// source text needs, then run it to completion on the 1937.
fn run_asm(body: &str) -> Pic14e {
    let src = format!(
        "STATUS equ 0x003\n\
         FSR0L equ 0x004\n\
         FSR0H equ 0x005\n\
         FSR1L equ 0x006\n\
         FSR1H equ 0x007\n\
         {body}"
    );
    let words = assemble_pic14e(&src);
    let mut p = Pic14e::with_device(&device::PIC16F1937, words);
    p.run(10_000);
    p
}

#[test]
fn asrf_shifts_right_arithmetically_holding_the_sign() {
    // 0x84 = 1000 0100: ASRF -> 1100 0010 (sign held), C = 0 (LSb was 0).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x84\n\
             MOVWF 0x20\n\
             ASRF 0x20, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x20], 0xC2);
    assert_eq!(p.ram()[0x03] & 0b001, 0, "C = LSb of the source");
    // ASRF of 0x05 (0000 0101) -> 0000 0010, C = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x05\n\
             MOVWF 0x20\n\
             ASRF 0x20, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x02);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C gets the shifted-out bit");
}

#[test]
fn lslf_and_lsrf_shift_zeros_in() {
    // LSLF 0x83 -> 0x06, C = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x83\n\
             MOVWF 0x20\n\
             LSLF 0x20, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x20], 0x06, "LSLF shifts a 0 into bit 0");
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "LSLF C = bit 7 of source");
    // LSRF 0x83 -> 0x41, C = 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x83\n\
             MOVWF 0x20\n\
             LSRF 0x20, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x20], 0x41, "LSRF shifts a 0 into bit 7");
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "LSRF C = bit 0 of source");
}

#[test]
fn movlb_and_movlp_load_bsr_and_pclath() {
    // MOVLB 0x1F -> BSR = 31; MOVLP 0x7F -> PCLATH = 0x7F (all 7 bits).
    let p = run_asm(
        "    org 0x20\n\
             MOVLB 0x1F\n\
             MOVLP 0x7F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x08], 0x1F, "MOVLB loads BSR (byte 0x08)");
    assert_eq!(p.ram()[0x0A], 0x7F, "MOVLP loads all 7 PCLATH bits");
}

#[test]
fn bra_branches_by_its_relative_offset() {
    // The 9-bit field is a signed literal the hardware adds to PC+1
    // (DS41364E section 3.3.4); gpasm's numeric operand is the absolute
    // target, so the assembler encodes `target - (pc + 1)` into the
    // field. The pc assertions fail under either a wrong encoder
    // (field = operand) or a wrong sim reading (offset from pc, not pc+1).
    // BRA 3 from word 0 (field +2) skips both NOPs and lands on the MOVLW.
    let p = {
        let words = assemble_pic14e(
            "    org 0\n\
             BRA 0x03\n\
             NOP\n\
             NOP\n\
             MOVLW 0x2A\n\
             SLEEP\n",
        );
        let mut p = Pic14e::with_device(&device::PIC16F1937, words);
        p.run(10_000);
        p
    };
    assert_eq!(p.pc(), 0x04, "BRA 3 must land on the SLEEP's address");
    assert_eq!(p.w(), 0x2A, "BRA must jump over both NOPs");
    // Negative target: BRA 0x23 from word 0x24 (field -2) must land on the
    // SLEEP and halt with the pre-branch W. A wrong sim reading (field
    // applied from pc, not pc+1) lands back on the GOTO at 0x22 and loops
    // forever; a wrong encoder (field = operand, not target-(pc+1)) lands
    // at 0x48. Both fail the pc assertion.
    let p = run_asm(
        "    org 0x21\n\
             MOVLW 0x11\n\
             GOTO 0x24\n\
             SLEEP\n\
             BRA 0x23\n",
    );
    assert_eq!(
        p.pc(),
        0x23,
        "BRA 0x23 must land on the SLEEP, negative field"
    );
    assert_eq!(p.w(), 0x11, "the pre-branch MOVLW's W survives");
}

#[test]
fn brw_branches_by_w() {
    // W = 2: BRW lands at PC + 1 + 2.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x02\n\
             BRW\n\
             MOVLW 0x11\n\
             MOVLW 0x22\n\
             MOVLW 0x33\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x33, "BRW skips two instructions after its own");
}

#[test]
fn callw_calls_through_w_and_pclath() {
    // PCLATH = 0x00 (MOVLP 0), W = 0x24: CALLW -> PC = 0x24; the callee
    // returns to the instruction after the CALLW.
    let p = run_asm(
        "    org 0x20\n\
             MOVLP 0\n\
             MOVLW 0x24\n\
             CALLW\n\
             SLEEP\n\
             MOVLW 0x2A\n\
             RETURN\n",
    );
    assert_eq!(p.w(), 0x2A, "CALLW jumps to W|PCLATH and RETURN comes back");
}

#[test]
fn addfsr_adds_signed_literals() {
    // FSR0 = 0x1000, ADDFSR FSR0, -1 -> 0x0FFF; FSR1 starts at 0 and
    // ADDFSR FSR1, 0x1F -> 0x001F.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x00\n\
             MOVWF FSR0L\n\
             MOVLW 0x10\n\
             MOVWF FSR0H\n\
             ADDFSR FSR0, -1\n\
             ADDFSR FSR1, 0x1F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x04], 0xFF, "FSR0L after -1");
    assert_eq!(p.ram()[0x05], 0x0F, "FSR0H after -1");
    assert_eq!(p.ram()[0x06], 0x1F, "FSR1L after +0x1F");
}

#[test]
fn moviw_movwi_pre_post_inc_dec_forms() {
    // Distinct values per address so each modifier form's read/write and
    // FSR side effect is individually observable: RAM[0x30]=0x10,
    // RAM[0x31]=0x20, RAM[0x32]=0x30; FSR0 = 0x0030. Each form is stepped
    // and checked before the next runs.
    let words = assemble_pic14e(
        "STATUS equ 0x003\n\
         FSR0L equ 0x004\n\
         FSR0H equ 0x005\n\
         FSR1L equ 0x006\n\
         FSR1H equ 0x007\n\
             org 0x20\n\
             MOVLW 0x10\n\
             MOVWF 0x30\n\
             MOVLW 0x20\n\
             MOVWF 0x31\n\
             MOVLW 0x30\n\
             MOVWF 0x32\n\
             MOVLW 0x30\n\
             MOVWF FSR0L\n\
             MOVLW 0x00\n\
             MOVWF FSR0H\n\
             MOVIW ++FSR0\n\
             MOVWI FSR0++\n\
             MOVWI --FSR0\n\
             MOVIW FSR0--\n\
             SLEEP\n",
    );
    let mut p = Pic14e::with_device(&device::PIC16F1937, words);
    // The program starts at org 0x20 (32 zero-padding words), then five
    // MOVLW/MOVWF pairs: 42 steps to reach the first MOVIW with FSR0 set.
    p.run(42);
    assert_eq!(p.ram()[0x04], 0x30, "FSR0L setup");
    // MOVIW ++FSR0: FSR0 0x30 -> 0x31, W = RAM[0x31] = 0x20.
    p.step();
    assert_eq!(p.w(), 0x20, "pre-increment read RAM[0x31]");
    assert_eq!(p.ram()[0x04], 0x31, "FSR0 advanced to 0x31");
    // MOVWI FSR0++: W (0x20) -> RAM[0x31], FSR0 -> 0x32.
    p.step();
    assert_eq!(p.ram()[0x31], 0x20, "post-increment write to RAM[0x31]");
    assert_eq!(p.ram()[0x04], 0x32, "FSR0 after post-increment");
    // MOVWI --FSR0: FSR0 0x32 -> 0x31, W (0x20) -> RAM[0x31].
    p.step();
    assert_eq!(p.ram()[0x04], 0x31, "pre-decrement FSR0");
    assert_eq!(p.ram()[0x31], 0x20, "pre-decrement write");
    // MOVIW FSR0--: W = RAM[0x31] = 0x20, FSR0 -> 0x30.
    p.step();
    assert_eq!(p.w(), 0x20, "post-decrement read RAM[0x31]");
    assert_eq!(p.ram()[0x04], 0x30, "FSR0 after post-decrement");
}

#[test]
fn moviw_indexed_reads_and_writes_with_signed_offset() {
    // FSR1 = 0x0030; RAM[0x34] = 0x99 (the value the read must surface);
    // MOVIW 4[FSR1] reads RAM[0x34]; MOVWI -2[FSR1] writes W to RAM[0x2E].
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x30\n\
             MOVWF FSR1L\n\
             MOVLW 0x00\n\
             MOVWF FSR1H\n\
             MOVLW 0x99\n\
             MOVWF 0x34\n\
             MOVLW 0x77\n\
             MOVIW 4[FSR1]\n\
             MOVWI -2[FSR1]\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x99, "indexed read got RAM[0x34]");
    assert_eq!(p.ram()[0x2E], 0x99, "indexed write went to RAM[0x2E]");
}

#[test]
fn option_writes_optreg_and_tris_writes_trisx() {
    // OPTION: W -> OPTION_REG (0x095); TRIS 5/6/7: W -> TRISA/B/C
    // (0x08C/0x08D/0x08E), DS41364E Register 12-3.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xA5\n\
             OPTION\n\
             MOVLW 0xF0\n\
             TRIS 5\n\
             MOVLW 0x0F\n\
             TRIS 6\n\
             MOVLW 0x3C\n\
             TRIS 7\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x95], 0xA5, "OPTION loads OPTION_REG");
    assert_eq!(p.ram()[0x8C], 0xF0, "TRIS 5 loads TRISA");
    assert_eq!(p.ram()[0x8D], 0x0F, "TRIS 6 loads TRISB");
    assert_eq!(p.ram()[0x8E], 0x3C, "TRIS 7 loads TRISC");
}

#[test]
fn status_bits_5_to_7_are_inert() {
    // STATUS bits 7-5 are unimplemented (read as 0, DS41364E Register 3-1):
    // a BSF/BCF against them must not change the register, and the read
    // back is 0 regardless of the write.
    let p = run_asm(
        "    org 0x20\n\
             BSF STATUS, 7\n\
             BSF STATUS, 6\n\
             BSF STATUS, 5\n\
             BCF STATUS, 6\n\
             SLEEP\n",
    );
    assert_eq!(
        p.ram()[0x03] & 0xE0,
        0,
        "bits 7-5 read as 0 even after BSF/BCF"
    );
}

#[test]
fn linear_region_aliases_the_banked_gpr() {
    // Linear address 0x2000 aliases bank 0's GPR at 0x20 (DS41364E
    // section 3.5.2): a store to linear 0x200F must land in RAM[0x2F].
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x00\n\
             MOVWF FSR0L\n\
             MOVLW 0x20\n\
             MOVWF FSR0H\n\
             MOVLW 0x4D\n\
             MOVWI 0x0F[FSR0]\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x2F], 0x4D, "linear 0x200F aliases bank-0 GPR 0x2F");
}

#[test]
fn program_flash_reads_low_byte_and_costs_an_extra_cycle() {
    // FSR0H MSb set: the FSR addresses program flash (DS41364E section
    // 3.5.3); a MOVIW reads the low 8 bits of the flash word. The read
    // also costs one extra cycle. The program runs from word 0; the flash
    // word under test sits at word 0x0020.
    let mut prog = assemble_pic14e(
        "FSR0L equ 0x004\n\
         FSR0H equ 0x005\n\
             org 0\n\
             MOVLW 0x20\n\
             MOVWF FSR0L\n\
             MOVLW 0x80\n\
             MOVWF FSR0H\n\
             MOVIW FSR0++\n\
             SLEEP\n",
    );
    prog.resize(0x21, 0);
    prog[0x20] = 0x1234; // flash word at 0x0020: low byte 0x34
    let mut p = Pic14e::with_device(&device::PIC16F1937, prog);
    // Four setup steps: MOVLW/MOVWF/MOVLW/MOVWF -> FSR0 = 0x8020.
    p.run(4);
    assert_eq!(p.ram()[0x05], 0x80, "FSR0H MSb set");
    // MOVIW executes; the flash word read lands in W and the next step is
    // consumed by the extra cycle.
    p.step(); // MOVIW: W = 0x34
    assert_eq!(p.w(), 0x34, "low 8 bits of flash word 0x0020");
    p.step(); // the flash access's extra cycle
    assert_eq!(p.pc(), 0x05, "extra cycle before SLEEP");
}

#[test]
fn addwfc_and_subwfb_set_carry_and_dc() {
    // ADDWFC with C set: 0xFF + 0x01 + 1 -> 0x01, C = 1, DC = 1 (low
    // nibble 0xF + 1 + 1).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0xFF\n\
             MOVWF 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x03\n\
             MOVLW 0x01\n\
             ADDWFC 0x20, F\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x20], 0x01);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = carry out of 0xFF+1+1");
    assert_eq!(
        p.ram()[0x03] & 0b010,
        0b010,
        "DC = carry out of the low nibble"
    );
    // SUBWFB with C set (no borrow): 0x2F - 0x11 - 0 -> 0x1E, C = 1 (no
    // borrow), DC = 1 (no nibble borrow).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x2F\n\
             MOVWF 0x20\n\
             MOVLW 0x01\n\
             MOVWF 0x03\n\
             MOVLW 0x11\n\
             SUBWFB 0x20, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x1E);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = 1 when no borrow");
    assert_eq!(p.ram()[0x03] & 0b010, 0b010, "DC = 1 when no nibble borrow");
    // SUBWFB with C clear (borrow): 0x05 - 0x07 - 1 -> 0xFD, C = 0, DC = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x05\n\
             MOVWF 0x20\n\
             MOVLW 0x07\n\
             SUBWFB 0x20, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0xFD);
    assert_eq!(p.ram()[0x03] & 0b001, 0, "C = 0 when a borrow occurs");
    assert_eq!(
        p.ram()[0x03] & 0b010,
        0,
        "DC = 0 when the low nibble borrows"
    );
}

#[test]
fn classic_opcodes_still_work_on_pic14e() {
    // The 14-bit PIC14 opcodes must behave identically on the enhanced
    // core: SUBWF (with DC), ADDLW (with DC), and CALL/RETLW (with the
    // pre-emption of 0x000A-0x000B by CALLW/BRW verified separately).
    // SUBWF f, W: f - W; DC = 1 while no nibble borrow (0x2 - 0x2).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x12\n\
             MOVWF 0x20\n\
             MOVLW 0x02\n\
             SUBWF 0x20, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x10);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001, "C = 1 while no borrow");
    assert_eq!(
        p.ram()[0x03] & 0b010,
        0b010,
        "DC = 1 while no nibble borrow"
    );
    // 0x12 - 0x03 = 0x0F: the low nibble borrows, DC = 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x12\n\
             MOVWF 0x20\n\
             MOVLW 0x03\n\
             SUBWF 0x20, W\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x0F);
    assert_eq!(
        p.ram()[0x03] & 0b010,
        0,
        "DC = 0 when the low nibble borrows"
    );
    // ADDLW with DC: 0x0F + 0x01 -> 0x10.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x0F\n\
             ADDLW 0x01\n\
             SLEEP\n",
    );
    assert_eq!(p.w(), 0x10);
    assert_eq!(
        p.ram()[0x03] & 0b010,
        0b010,
        "ADDLW sets DC on nibble carry"
    );
    // CALL/RETLW round trip through the stack.
    let p = run_asm(
        "    org 0x20\n\
             CALL 0x30\n\
             SLEEP\n\
             org 0x30\n\
             RETLW 0x2A\n",
    );
    assert_eq!(p.w(), 0x2A, "RETLW loads W and returns past the CALL");
    assert_eq!(
        p.pc(),
        0x21,
        "the RETLW returns to the SLEEP after the CALL"
    );
}

#[test]
fn direct_operands_page_by_bsr() {
    // With BSR = 1, a direct operand 0x20 addresses physical 0xA0
    // (DS41364E section 3.2: 32 banks x 128 bytes selected by BSR).
    let p = run_asm(
        "    org 0x20\n\
             MOVLB 1\n\
             MOVLW 0x5A\n\
             MOVWF 0x20\n\
             SLEEP\n",
    );
    assert_eq!(
        p.ram()[0xA0],
        0x5A,
        "direct 0x20 with BSR=1 is physical 0xA0"
    );
    assert_eq!(p.ram()[0x20], 0, "bank-0 0x20 untouched");
}

#[test]
fn common_ram_mirrors_across_bsr() {
    // The 16 bytes 0x70-0x7F are common RAM, reachable from any bank at
    // their physical address regardless of BSR (DS41364E section 3.2.4).
    // With BSR = 1 the direct 0x72 must still land on physical 0x72, not
    // page to 0xF2 (surfaced by the cross-bank __mul_u16 recipe in the P2
    // e2e: its retval region 0x71-0x74 would be corrupted by a nonzero BSR
    // if common RAM were paged).
    let p = run_asm(
        "    org 0x20\n\
             MOVLB 1\n\
             MOVLW 0x5A\n\
             MOVWF 0x72\n\
             SLEEP\n",
    );
    assert_eq!(
        p.ram()[0x72],
        0x5A,
        "direct 0x72 with BSR=1 is the common-RAM physical 0x72"
    );
    assert_eq!(
        p.ram()[0xF2],
        0,
        "the paged 0xF2 must not receive the write"
    );
}
