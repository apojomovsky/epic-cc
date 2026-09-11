//! SPIKE for epic-cc#393 D-2: does docs/39 section 3's shadow-slot ISR
//! technique actually work for a device shaped like PIC16F74 (two real,
//! non-aliased GPR regions, `common_ram = None`)? Not wired into isel; this
//! hand-assembles the exact instruction sequence and drives it through the
//! real simulator, the same way `gpr_window.rs` validates addressing with a
//! synthetic `Device` and no codegen involved.
//!
//! Finding: docs/39 section 3 step 3's prose ("MOVWF STATUS directly from
//! the saved (still-swapped) byte") is imprecise. Nibble-swapping STATUS
//! once (to capture it bank-independently) and then writing that swapped
//! byte straight back does NOT restore STATUS -- it scrambles every bit
//! into the wrong position (Z ends up where RP1 was, C where TO was, etc).
//! The real, working idiom -- already used by this repo's own bucket-1
//! (common_ram-backed) ISR prologue at crates/isel/src/lib.rs's `SWAPF
//! 0x76, W` / `MOVWF STATUS` -- swaps TWICE: once to capture (flag-safe
//! read), once more to un-swap at restore, immediately before `MOVWF
//! STATUS`. This test implements the corrected sequence; a sibling
//! assertion below documents the broken one failing.

use device::{ConfigRegion, Core, Device};
use pic14_sim::Pic14;

// MOVLW k = 0x3000 | k. MOVWF f = 0x0080 | f. MOVF f,W = 0x0800 | f.
// SWAPF f,W = 0x0E00 | f; SWAPF f,F = 0x0E80 | f. BCF f,b = 0x1000 | b<<7 | f;
// BSF f,b = 0x1400 | b<<7 | f. ADDLW k = 0x3E00 | k. GOTO k = 0x2800 | k.
// RETFIE = 0x0009, SLEEP = 0x0063. STATUS = 0x03, RP0 = bit 5, RP1 = bit 6.
const STATUS: u16 = 0x03;
const RP0: u16 = 5;
const RP1: u16 = 6;

fn bcf(f: u16, b: u16) -> u16 {
    0x1000 | (b << 7) | f
}
fn bsf(f: u16, b: u16) -> u16 {
    0x1400 | (b << 7) | f
}
fn movwf(f: u16) -> u16 {
    0x0080 | f
}
fn swapf_w(f: u16) -> u16 {
    0x0E00 | f
}
fn swapf_f(f: u16) -> u16 {
    0x0E80 | f
}
fn movlw(k: u16) -> u16 {
    0x3000 | k
}
fn addlw(k: u16) -> u16 {
    0x3E00 | k
}
fn goto(k: u16) -> u16 {
    0x2800 | k
}
const RETFIE: u16 = 0x0009;
const SLEEP: u16 = 0x0063;

/// PIC16F74's real shape (docs/39 section 1 bucket 2): two distinct,
/// non-aliased 96-byte GPR regions, `common_ram = None`. Region A
/// (0x20-0x7F) is bank 0 (RP1:RP0 = 00); region B (0xA0-0xFF) is bank 1
/// (RP0 = 1, RP1 = 0) -- confirmed via `Pic14::bank_base`'s
/// `((STATUS>>5)&3)*0x80` encoding: bank index 1 lands exactly on
/// 0x20+0x80=0xA0. Docs/39's prose attributes the region split to RP1;
/// the working bit is actually RP0 for this specific pair of banks --
/// immaterial to the fix itself, which clears both bits and so is
/// correct regardless of which one is architecturally live here, but
/// worth a doc fix given this test needed the real bit to build a
/// program at all.
const F74_SHAPE: Device = Device {
    name: "spike-f74",
    core: Core::Pic14,
    flash_words: 0x800,
    ram_banks: &[(0x20, 0x7F), (0xA0, 0xFF)],
    common_ram: None,
    access_bank: None,
    fixed_retval: None,
    isr_w_shadow: None,
    isr_home_window: None,
    stack_depth: 8,
    fsr_bank_bits: 0,
    interrupt_vectors: &[0x0004],
    config: ConfigRegion {
        base_byte_addr: 0x400E,
        num_bytes: 2,
        erased_baseline: &[0xFF, 0x3F],
        fields: &[],
    },
    sfrs: &[],
};

/// Offset 0x20 (W shadow) and 0x21 (STATUS shadow, region-A-only) are
/// reserved out of both regions' normal allocation pool per docs/39
/// section 3 step 1 -- the test's "main program" below deliberately uses
/// 0x22/0x23 instead so a collision would be obvious as a wrong-value
/// assertion, not a silent overlap.
const W_SHADOW: u16 = 0x20;
const STATUS_SHADOW: u16 = 0x21;

/// The correct shadow-slot prologue/epilogue (docs/39 section 3, with the
/// missing un-swap restored -- see module docs). Occupies words 4..=18;
/// word 4 is the fixed PIC14 interrupt vector.
fn isr_correct() -> Vec<u16> {
    vec![
        // prologue
        movwf(W_SHADOW),      // 4: W_TEMP (region-relative) = W, raw
        swapf_f(W_SHADOW),    // 5: W_TEMP = swap(W_TEMP), in place, same bank as entry
        swapf_w(STATUS),      // 6: W = swap(STATUS), bank-independent SFR, flag-safe
        bcf(STATUS, RP1),     // 7: force RP1=0
        bcf(STATUS, RP0),     // 8: force RP0=0 -> region A
        movwf(STATUS_SHADOW), // 9: STATUS_TEMP (region-A-only) = swap(STATUS), now safe
        // disruptive ISR body: clobber W, flags, and switch banks
        movlw(0x00),      // 10
        addlw(0xFF),      // 11: W=0xFF, Z=0,C=0,DC=0
        bsf(STATUS, RP1), // 12: bank -> 2 (neither region A nor B)
        // epilogue
        bcf(STATUS, RP1),       // 13: force region A again
        bcf(STATUS, RP0),       // 14
        swapf_w(STATUS_SHADOW), // 15: W = swap(STATUS_TEMP) = original STATUS (un-swap)
        movwf(STATUS),          // 16: STATUS = original -> also re-selects the original region
        swapf_w(W_SHADOW),      // 17: W = swap(W_TEMP) = original W (region now correct again)
        RETFIE,                 // 18
    ]
}

/// docs/39 section 3 step 3 taken completely literally: skip the restore
/// un-swap and `MOVWF STATUS` straight from the once-swapped byte. Same
/// prologue, broken epilogue -- kept short since it only needs to prove
/// the restore is wrong, not survive a full run.
fn isr_broken_restore() -> Vec<u16> {
    vec![
        movwf(W_SHADOW),      // 4
        swapf_f(W_SHADOW),    // 5
        swapf_w(STATUS),      // 6
        bcf(STATUS, RP1),     // 7
        bcf(STATUS, RP0),     // 8
        movwf(STATUS_SHADOW), // 9
        bcf(STATUS, RP1),     // 10: force region A for restore
        bcf(STATUS, RP0),     // 11
        movwf(STATUS),        // 12: WRONG -- no un-swap first (the doc's literal text)
        RETFIE,               // 13
    ]
}

fn main_program(isr_len: usize) -> Vec<u16> {
    let main_start = 4 + isr_len;
    let mut prog = vec![goto(main_start as u16), 0, 0, 0];
    // Pad through the vector/ISR region (words 4..main_start); the caller
    // splices the real ISR words over this range afterward.
    prog.resize(main_start, 0);
    prog.extend([
        bsf(STATUS, RP0), // select region B
        movlw(0xFF),
        addlw(0x02), // W=0x01, C=1, DC=1, Z=0
        movwf(0x22), // "live" region-B variable, distinct from the reserved slots
        // <-- interrupt injected here, pc == main_start + 4
        movwf(0x23), // post-return marker: store whatever W now holds
        SLEEP,
    ]);
    prog
}

#[test]
fn shadow_slot_technique_restores_w_status_and_bank_across_a_region_switch() {
    let isr = isr_correct();
    let isr_len = isr.len();
    let mut prog = main_program(isr_len);
    // Splice the ISR into words 4..4+isr_len, overwriting the padding.
    for (i, w) in isr.iter().enumerate() {
        prog[4 + i] = *w;
    }

    let mut p = Pic14::with_device(&F74_SHAPE, prog);
    // Run the 4 "before" main words: GOTO, BSF RP0, MOVLW, ADDLW, MOVWF 0x22.
    for _ in 0..5 {
        p.step();
    }
    let injection_pc = p.pc();
    let w_before = p.w();
    let status_before = p.status();
    let bank_before = p.bank();
    assert_eq!(w_before, 0x01, "setup: W must be the ADDLW result");
    assert_eq!(bank_before, 1, "setup: region B is bank 1 (RP0 set)");
    assert_ne!(status_before & 0x01, 0, "setup: carry must be set");
    assert_ne!(status_before & 0x02, 0, "setup: DC must be set");
    assert_eq!(
        p.ram()[0xA2],
        0x01,
        "setup: the live variable landed in region B"
    );

    p.fire_interrupt();
    assert_eq!(p.pc(), 4, "vectors to word 4");

    // Run the whole ISR (prologue + disruptive body + epilogue + RETFIE)
    // plus the two post-return main words, then confirm it halted rather
    // than looping or crashing.
    p.run(100);
    assert!(p.halted(), "must reach SLEEP, not get lost mid-restore");

    assert_eq!(
        p.pc(),
        injection_pc + 1,
        "resumed exactly where it left off"
    );
    assert_eq!(
        p.w(),
        w_before,
        "W fully restored despite the ISR clobbering it to 0xFF"
    );
    assert_eq!(
        p.status(),
        status_before,
        "STATUS (bank bits + Z/C/DC) fully restored despite the ISR switching banks"
    );
    assert_eq!(
        p.bank(),
        bank_before,
        "back in region B, not wherever the ISR body left it"
    );
    assert_eq!(
        p.ram()[0xA3],
        w_before,
        "the post-return instruction saw the correctly restored W"
    );
    assert_eq!(
        p.ram()[0xA2],
        0x01,
        "the region-B live variable was never touched by the ISR"
    );
}

#[test]
fn skipping_the_restore_unswap_corrupts_status_as_docs_39_literally_describes_it() {
    // Confirms the failure mode, not just its absence: docs/39's literal
    // "MOVWF STATUS directly from the saved (still-swapped) byte" text
    // scrambles STATUS's bits into the wrong nibble positions rather than
    // restoring them, because a nibble-swapped byte written straight back
    // is not the identity transform SWAPF-then-SWAPF is.
    let isr = isr_broken_restore();
    let isr_len = isr.len();
    let mut prog = main_program(isr_len);
    for (i, w) in isr.iter().enumerate() {
        prog[4 + i] = *w;
    }

    let mut p = Pic14::with_device(&F74_SHAPE, prog);
    for _ in 0..5 {
        p.step();
    }
    let status_before = p.status();
    let bank_before = p.bank();

    p.fire_interrupt();
    // Run exactly through the broken ISR's RETFIE (9 words: 4..=12 inclusive
    // is 9 steps) without running past it into the (now bank-scrambled,
    // possibly nonsensical) resumed main code.
    for _ in 0..9 {
        p.step();
    }

    assert_ne!(
        p.status(),
        status_before,
        "the literal docs/39 restore step scrambles STATUS instead of restoring it \
         (swap-once-then-write-back is not the same as swap-capture then swap-restore)"
    );
    let _ = bank_before; // documented for readers comparing against the correct-path test
}
