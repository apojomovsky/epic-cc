//! The D-2 ordering-hazard acceptance test (docs/37 section 2 D-2, section
//! 3 P1): on baseline, `FSR<5>` is the bank select for *both* direct and
//! indirect addressing, and `FSR` is also the only indirect pointer. A
//! direct access to bank 1 followed by an indirect access through a live
//! pointer into bank 0 needs bank 0's bits re-asserted (`BCF FSR,5`)
//! before the `INDF` touch, and the reverse ordering needs `BSF FSR,5`.
//! This is the "not yet simulator-proven" gap D-2 flags; these tests prove
//! the `BCF`/`BSF FSR,5` reassertion sequence produces the right effective
//! addresses in `sim`.

use asm::assemble_pic_baseline;
use pic14_sim::PicBaseline;

/// Assemble `body` (org-anchored) with the core register symbols, then run
/// it to completion on the 509.
fn run_asm(body: &str) -> PicBaseline {
    let src = format!(
        "INDF   equ 0x000\n\
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
fn direct_bank1_then_indirect_through_live_pointer_into_bank0() {
    // Direct access to bank 1 (BSF FSR,5; MOVWF 0x10 -> physical 0x30),
    // then an indirect access through a live pointer into bank 0. The
    // pointer load (MOVLW 0x10; MOVWF FSR) sets FSR<5> = 0, so the INDF
    // read must land in bank 0 (0x10), not bank 1 (0x30).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x5A\n\
             BSF FSR, 5\n\
             MOVWF 0x10\n\
             MOVLW 0x10\n\
             MOVWF FSR\n\
             MOVF INDF, W\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x30], 0x5A, "the direct write landed in bank 1");
    assert_eq!(
        p.ram()[0x10],
        0x00,
        "bank 0 GPR untouched by the direct write"
    );
    assert_eq!(p.w(), 0x00, "INDF reads bank 0 (0x10), not bank 1 (0x30)");
}

#[test]
fn direct_bank0_then_indirect_through_live_pointer_into_bank1() {
    // The reverse ordering: a direct access to bank 0, then an indirect
    // access through a pointer into bank 1. The pointer load sets
    // FSR<5> = 1, so the INDF read must land in bank 1 (0x30).
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x6B\n\
             BCF FSR, 5\n\
             MOVWF 0x10\n\
             MOVLW 0x30\n\
             MOVWF FSR\n\
             MOVF INDF, W\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x6B, "the direct write landed in bank 0");
    assert_eq!(
        p.ram()[0x30],
        0x00,
        "bank 1 GPR untouched by the direct write"
    );
    assert_eq!(p.w(), 0x00, "INDF reads bank 1 (0x30), not bank 0 (0x10)");
}

#[test]
fn indirect_through_live_pointer_then_direct_access_reasserts_bank() {
    // An indirect access through a pointer into bank 1, then a direct
    // access to bank 0. The direct access must re-assert FSR<5> = 0
    // (BCF FSR,5) before the MOVWF, or it would land in bank 1.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x30\n\
             MOVWF FSR\n\
             MOVLW 0x7C\n\
             MOVWF INDF\n\
             BCF FSR, 5\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x30], 0x7C, "the indirect write landed in bank 1");
    assert_eq!(
        p.ram()[0x10],
        0x5A,
        "the direct write landed in bank 0 after BCF FSR,5"
    );
}

#[test]
fn indirect_through_live_pointer_then_direct_access_to_bank1() {
    // An indirect access through a pointer into bank 0, then a direct
    // access to bank 1. The direct access must re-assert FSR<5> = 1
    // (BSF FSR,5) before the MOVWF, or it would land in bank 0.
    let p = run_asm(
        "    org 0x20\n\
             MOVLW 0x10\n\
             MOVWF FSR\n\
             MOVLW 0x7C\n\
             MOVWF INDF\n\
             BSF FSR, 5\n\
             MOVLW 0x5A\n\
             MOVWF 0x10\n\
             SLEEP\n",
    );
    assert_eq!(p.ram()[0x10], 0x7C, "the indirect write landed in bank 0");
    assert_eq!(
        p.ram()[0x30],
        0x5A,
        "the direct write landed in bank 1 after BSF FSR,5"
    );
}
