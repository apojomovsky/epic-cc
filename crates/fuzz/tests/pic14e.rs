//! PIC14E differential gate (P8): the same seeded corpora as the PIC14
//! gate, but threaded through `&device::PIC16F1937`. The fast subsets
//! (seeds 0..8) run as normal tests; the full corpora run under
//! `--ignored` and must be clean. Gaps closed in P6/P7, so the fast gate
//! is now strict: every seed must be clean (no skipped tolerance).

use device::PIC16F1937;
use fuzz::{
    generate, generate_float, generate_ir, generate_signed, run_differential,
    run_differential_with_profile, run_ir_differential, run_ir_twin_differential_with_profile,
};

#[test]
fn pic14e_integer_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e integer seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 integer seeds clean");
}

#[test]
fn pic14e_integer_fast_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the PIC14E integer gate (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e integer seed {seed} not differential-clean under -O2: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 integer seeds clean under -O2");
}

#[test]
fn pic14e_float_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_float(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e float seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 float seeds clean");
}

#[test]
fn pic14e_float_fast_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the PIC14E float gate (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_float(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e float seed {seed} not differential-clean under -O2: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 float seeds clean under -O2");
}

#[test]
fn pic14e_signed_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_signed(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e signed seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 signed seeds clean");
}

#[test]
fn pic14e_signed_fast_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the PIC14E signed gate (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_signed(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e signed seed {seed} not differential-clean under -O2: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 signed seeds clean under -O2");
}

#[test]
fn pic14e_ir_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_ir(seed);
        match run_ir_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e IR seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 IR seeds clean");
}

#[test]
fn pic14e_ir_fast_corpus_differential_clean_under_o2() {
    // The `-O2` slice for the PIC14E IR fast seeds (epic-cc#860): the C
    // twins through the speed profile (the canonical path is
    // profile-free; see `run_ir_twin_differential_with_profile`).
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_ir(seed);
        match run_ir_twin_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic14e IR seed {seed} not differential-clean under -O2: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 IR seeds clean under -O2");
}

#[test]
#[ignore = "full 200-seed pic14e integer corpus (slow)"]
fn pic14e_full_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..200 {
        let prog = generate(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(f) => panic!("pic14e full corpus seed {seed} failed ({:?}): {f}", f.kind),
        }
    }
    assert_eq!(clean, 200);
}

#[test]
#[ignore = "full 200-seed pic14e integer corpus under -O2 (slow)"]
fn pic14e_full_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the full PIC14E integer corpus (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..200 {
        let prog = generate(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(f) => panic!(
                "pic14e full corpus seed {seed} failed under -O2 ({:?}): {f}",
                f.kind
            ),
        }
    }
    assert_eq!(clean, 200);
}

#[test]
#[ignore = "full 50-seed pic14e float corpus (slow)"]
fn pic14e_float_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_float(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(f) => panic!("pic14e float corpus seed {seed} failed ({:?}): {f}", f.kind),
        }
    }
    assert_eq!(clean, 50);
}

#[test]
#[ignore = "full 50-seed pic14e float corpus under -O2 (slow)"]
fn pic14e_float_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the full PIC14E float corpus (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_float(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(f) => panic!(
                "pic14e float corpus seed {seed} failed under -O2 ({:?}): {f}",
                f.kind
            ),
        }
    }
    assert_eq!(clean, 50);
}

#[test]
#[ignore = "full 50-seed pic14e signed corpus (slow)"]
fn pic14e_signed_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_signed(seed);
        match run_differential(&prog, &PIC16F1937) {
            Ok(_) => clean += 1,
            Err(f) => panic!(
                "pic14e signed corpus seed {seed} failed ({:?}): {f}",
                f.kind
            ),
        }
    }
    assert_eq!(clean, 50);
}

#[test]
#[ignore = "full 50-seed pic14e signed corpus under -O2 (slow)"]
fn pic14e_signed_corpus_differential_clean_under_o2() {
    // The `-O2` slice of the full PIC14E signed corpus (epic-cc#860).
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_signed(seed);
        match run_differential_with_profile(&prog, &PIC16F1937, Some("O2")) {
            Ok(_) => clean += 1,
            Err(f) => panic!(
                "pic14e signed corpus seed {seed} failed under -O2 ({:?}): {f}",
                f.kind
            ),
        }
    }
    assert_eq!(clean, 50);
}
