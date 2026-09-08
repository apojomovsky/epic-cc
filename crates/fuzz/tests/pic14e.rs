//! PIC14E differential gate (P8): the same seeded corpora as the PIC14
//! gate, but threaded through `&device::PIC16F1937`. The fast subsets
//! (seeds 0..8) run as normal tests; the full corpora run under
//! `--ignored` and must be clean. The PIC14E port's P2-P7 phases closed
//! the integer/float/signed/IR gaps, so the fast gate is strict: every
//! seed must be clean (no skipped tolerance).

use device::PIC16F1937;
use fuzz::{
    generate, generate_float, generate_ir, generate_signed, run_differential, run_ir_differential,
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
