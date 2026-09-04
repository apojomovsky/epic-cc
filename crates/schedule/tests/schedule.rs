use device::PIC16F877A;
use schedule::{classify, phase1, phase2, regions, schedule, Line};

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

// -- phase 1: the singleton-excursion swap (ADR-027) --------------------

#[test]
fn sinks_a_w_and_flag_free_excursion_past_its_successor() {
    // A, cur(B), A: cur is BSF, which touches neither W nor a flag, so
    // it is a move candidate; its successor is not a skip op and shares
    // no file address with it, so sink succeeds.
    let asm = "    MOVWF 0x20\n    BSF 0x85, 3\n    MOVWF 0x21\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 1);
    let order: Vec<&str> = lines.iter().map(Line::raw).collect();
    assert_eq!(
        order,
        vec!["    MOVWF 0x20", "    MOVWF 0x21", "    BSF 0x85, 3"],
        "cur sank past its successor, merging the two bank-0 operands"
    );
}

#[test]
fn falls_back_to_hoisting_when_the_successor_is_a_skip_op() {
    // Sink is blocked (the successor is a skip op: swapping into that
    // slot would corrupt what it guards), so phase1 falls back to
    // hoisting cur past its predecessor instead.
    let asm = "    MOVWF 0x20\n    BSF 0x85, 3\n    BTFSC 0x21, 2\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 1);
    let order: Vec<&str> = lines.iter().map(Line::raw).collect();
    assert_eq!(
        order,
        vec!["    BSF 0x85, 3", "    MOVWF 0x20", "    BTFSC 0x21, 2"],
        "cur hoisted past its predecessor instead"
    );
}

#[test]
fn declines_when_the_excursion_instruction_reads_w() {
    // The exact ADR-027 EPIC_IRQ_Enable shape: cur is a MOVWF, which
    // always reads W, so it is never a phase-1 move candidate. The real
    // fix for this shape moves a different, independent instruction
    // instead, deferred to a later phase, not silently attempted here.
    let asm = "    MOVWF 0x190\n    MOVWF 0x8D\n    MOVWF 0x192\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 0, "MOVWF reads W, never a move candidate");
}

#[test]
fn declines_when_the_excursion_instruction_sets_flags() {
    let asm = "    MOVWF 0x20\n    ANDLW 0x0F\n    MOVWF 0x21\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 0, "ANDLW touches both W and flags");
}

#[test]
fn declines_a_skip_target_excursion_instruction() {
    // cur is itself the atomic other half of a skip pair: never a move
    // candidate regardless of what it otherwise is.
    let asm = "    BTFSC 0x20, 2\n    BSF 0x85, 3\n    MOVWF 0x21\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 0, "cur is a skip target");
}

#[test]
fn declines_a_non_excursion_run_of_one_bank() {
    // No excursion at all: every operand needs the same bank, so there
    // is nothing to reduce.
    let asm = "    MOVWF 0x20\n    MOVWF 0x21\n    MOVWF 0x22\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 0);
}

#[test]
fn phase1_never_crosses_a_region_boundary() {
    // The excursion candidate at index 1 has a label right after it:
    // there is no `i + 1` inside the same region, so it must never move.
    let asm = "    MOVWF 0x20\n    BSF 0x85, 3\nl1:\n    MOVWF 0x21\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase1(&mut lines);
    assert_eq!(n, 0);
}

#[test]
fn schedule_applies_phase1_end_to_end() {
    let asm = "    MOVWF 0x20\n    BSF 0x85, 3\n    MOVWF 0x21\n";
    let out = schedule(&PIC16F877A, asm);
    assert_eq!(out, "    MOVWF 0x20\n    MOVWF 0x21\n    BSF 0x85, 3\n");
}

// -- phase 2: the dead-W bundle hoist (ADR-027, the corrected version) --

/// The corrected `EPIC_IRQ_Enable` shape: `cur` (`MOVWF 0x85`) reads W,
/// so phase 1 declines it. `MOVF 0x21, W` (index 1 of the preceding run)
/// does not read W, a genuine dead-W gap; `(MOVLW 0x40, MOVWF 0x23)`
/// splices in right before it.
fn irq_enable_shaped_asm() -> &'static str {
    "    MOVWF 0x20\n\
     \x20   MOVF 0x21, W\n\
     \x20   IORWF 0x20, W\n\
     \x20   MOVWF 0x22\n\
     \x20   MOVWF 0x85\n\
     \x20   MOVLW 0x40\n\
     \x20   MOVWF 0x23\n"
}

#[test]
fn phase2_splices_a_bundle_into_a_genuine_dead_w_gap() {
    let mut lines = classify(&PIC16F877A, irq_enable_shaped_asm());
    let n = phase2(&mut lines);
    assert_eq!(n, 1);
    let order: Vec<&str> = lines.iter().map(Line::raw).collect();
    assert_eq!(
        order,
        vec![
            "    MOVWF 0x20",
            "    MOVLW 0x40",
            "    MOVWF 0x23",
            "    MOVF 0x21, W",
            "    IORWF 0x20, W",
            "    MOVWF 0x22",
            "    MOVWF 0x85",
        ],
        "the bundle spliced in before the first line that doesn't read W"
    );
}

#[test]
fn phase1_alone_does_not_touch_the_irq_enable_shape() {
    // Confirms phase 1's own documented boundary: cur reads W, so phase 1
    // must decline this shape entirely, leaving it for phase 2.
    let mut lines = classify(&PIC16F877A, irq_enable_shaped_asm());
    let n = phase1(&mut lines);
    assert_eq!(n, 0);
}

#[test]
fn phase2_declines_when_the_run_has_no_dead_w_gap() {
    // Every element of the preceding run reads W (ADDWF/IORWF-style
    // chains), so there is nowhere safe to splice the bundle in.
    let asm = "    MOVWF 0x20\n\
               \x20   IORWF 0x20, W\n\
               \x20   ADDWF 0x20, W\n\
               \x20   MOVWF 0x22\n\
               \x20   MOVWF 0x85\n\
               \x20   MOVLW 0x40\n\
               \x20   MOVWF 0x23\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase2(&mut lines);
    assert_eq!(n, 0);
}

#[test]
fn phase2_declines_when_something_after_the_bundle_still_reads_w() {
    let asm = "    MOVWF 0x20\n\
               \x20   MOVF 0x21, W\n\
               \x20   IORWF 0x20, W\n\
               \x20   MOVWF 0x22\n\
               \x20   MOVWF 0x85\n\
               \x20   MOVLW 0x40\n\
               \x20   MOVWF 0x23\n\
               \x20   MOVWF 0x24\n"; // reads W: still needs what MOVLW set
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase2(&mut lines);
    assert_eq!(n, 0, "the trailing MOVWF still needs W == 0x40");
}

#[test]
fn phase2_declines_a_skip_target_excursion_instruction() {
    // cur (MOVWF 0x85) immediately follows a skip op (BTFSC 0x22, 2,
    // itself bank0 so it still completes the excursion pattern). cur
    // itself never moves in phase 2 (only P/M do), so this exclusion is
    // a conservative, uniform "never reason about anything skip-adjacent"
    // policy matching phase 1, not because this specific splice would
    // provably break the (BTFSC, cur) pair.
    let asm = "    MOVWF 0x20\n\
               \x20   MOVF 0x21, W\n\
               \x20   IORWF 0x20, W\n\
               \x20   BTFSC 0x22, 2\n\
               \x20   MOVWF 0x85\n\
               \x20   MOVLW 0x40\n\
               \x20   MOVWF 0x23\n";
    let mut lines = classify(&PIC16F877A, asm);
    let n = phase2(&mut lines);
    assert_eq!(n, 0, "cur is a skip target");
}

#[test]
fn schedule_applies_phase2_end_to_end() {
    let out = schedule(&PIC16F877A, irq_enable_shaped_asm());
    assert_eq!(
        out,
        "    MOVWF 0x20\n\
         \x20   MOVLW 0x40\n\
         \x20   MOVWF 0x23\n\
         \x20   MOVF 0x21, W\n\
         \x20   IORWF 0x20, W\n\
         \x20   MOVWF 0x22\n\
         \x20   MOVWF 0x85\n"
    );
}
