//! Differential-harness tests (Task 1): a tiny hand-written program must be
//! differential-clean across a few input seeds, a deliberately mismatching
//! variant (a C-discipline violation) must FAIL the comparison, and the
//! seeded generator must produce deterministic, differential-clean programs.

use fuzz::{generate, run_differential, Input, Program};

/// The brief's tiny program: one u8 volatile input, one scalar expression.
const TINY: &str = "volatile unsigned char in0;\n\
                    volatile unsigned char checksum;\n\
                    void main(void){ checksum = (unsigned char)(in0 * 7 + 3); }\n";

fn tiny_program(in0: u32) -> Program {
    Program {
        c_source: TINY.to_string(),
        inputs: vec![Input { name: "in0".into(), value: in0, width: 8 }],
        checksum_name: "checksum".into(),
    }
}

#[test]
fn tiny_program_differential_clean() {
    // (unsigned char)(in0 * 7 + 3) for in0 = 0, 1, 200.
    for (in0, expect) in [(0u32, 3u32), (1, 10), (200, 123)] {
        let got = run_differential(&tiny_program(in0)).unwrap_or_else(|e| {
            panic!("seed in0={in0} not differential-clean: {e}")
        });
        assert_eq!(got, expect, "checksum for in0={in0}");
    }
}

#[test]
fn mismatching_variant_fails() {
    // A discipline violation: `int` is 16-bit on msp430 but 32-bit on the
    // host, so `(int)in0 * 300` wraps to -5536 on the PIC (and clang folds
    // the comparison `-5536 > 40000` to constant 0) while the host keeps
    // 60000 (60000 > 40000 == 1). The harness MUST report the difference.
    let prog = Program {
        c_source: "volatile unsigned char in0;\n\
                   volatile unsigned char checksum;\n\
                   void main(void){ checksum = (unsigned char)((int)in0 * 300 > 40000); }\n"
            .to_string(),
        inputs: vec![Input { name: "in0".into(), value: 200, width: 8 }],
        checksum_name: "checksum".into(),
    };
    match run_differential(&prog) {
        Err(e) => assert!(e.contains("mismatch"), "expected a mismatch, got: {e}"),
        Ok(v) => panic!("expected a mismatch, got Ok({v})"),
    }
}

#[test]
fn generate_is_deterministic_and_disciplined() {
    let a = generate(42);
    let b = generate(42);
    assert_eq!(a.c_source, b.c_source, "same seed must give the same source");
    assert!(a.c_source.contains("volatile unsigned char in0;"), "volatile input decl");
    assert!(a.c_source.contains("(unsigned char)("), "explicit narrowing cast");
    assert!(a.c_source.ends_with("}\n"), "the generated main ends the file");
    assert_eq!(a.checksum_name, "checksum");
    assert!(!a.inputs.is_empty());
}

#[test]
fn generator_corpus_differential_clean() {
    // A fixed small seed set, differential-clean (the plan's fast subset).
    for seed in 0..8 {
        let prog = generate(seed);
        run_differential(&prog)
            .unwrap_or_else(|e| panic!("generated seed {seed} not differential-clean: {e}"));
    }
}

#[test]
#[ignore = "manual 20-seed smoke (slow)"]
fn generator_20_seed_smoke() {
    for seed in 0..20 {
        let prog = generate(seed);
        run_differential(&prog)
            .unwrap_or_else(|e| panic!("generated seed {seed} not differential-clean: {e}"));
    }
}
