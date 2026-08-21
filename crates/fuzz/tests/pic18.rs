//! PIC18 differential gate (P8): the same seeded corpora as the PIC14
//! gate, but threaded through `&device::PIC18F4550`. The fast subsets
//! (seeds 0..8) run as normal tests; the full corpora run under
//! `--ignored` and must be clean. Gaps closed in P6/P7, so the fast gate
//! is now strict: every seed must be clean (no skipped tolerance).

use device::PIC18F4550;
use fuzz::{
    generate, generate_float, generate_ir, generate_signed, run_differential, run_ir_differential,
};

#[test]
fn pic18_integer_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic18 integer seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 integer seeds clean");
}

#[test]
fn pic18_float_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_float(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic18 float seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 float seeds clean");
}

#[test]
fn pic18_signed_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_signed(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic18 signed seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 signed seeds clean");
}

#[test]
fn pic18_ir_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_ir(seed);
        match run_ir_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic18 IR seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 IR seeds clean");
}

#[test]
#[ignore = "full 200-seed pic18 integer corpus (slow)"]
fn pic18_full_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..200 {
        let prog = generate(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(f) => panic!("pic18 full corpus seed {seed} failed ({:?}): {f}", f.kind),
        }
    }
    assert_eq!(clean, 200);
}

#[test]
#[ignore = "full 50-seed pic18 float corpus (slow)"]
fn pic18_float_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_float(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(f) => panic!("pic18 float corpus seed {seed} failed ({:?}): {f}", f.kind),
        }
    }
    assert_eq!(clean, 50);
}

#[test]
#[ignore = "full 50-seed pic18 signed corpus (slow)"]
fn pic18_signed_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_signed(seed);
        match run_differential(&prog, &PIC18F4550) {
            Ok(_) => clean += 1,
            Err(f) => panic!("pic18 signed corpus seed {seed} failed ({:?}): {f}", f.kind),
        }
    }
    assert_eq!(clean, 50);
}
