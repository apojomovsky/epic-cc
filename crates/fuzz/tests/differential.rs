//! Differential-harness tests (Task 1): a tiny hand-written program must be
//! differential-clean across a few input seeds, a deliberately mismatching
//! variant (a C-discipline violation) must FAIL the comparison, and the
//! seeded generator must produce deterministic, differential-clean programs.
//!
//! Task 2: the explicit-width typedef discipline (a u32 value > 2^16 wraps
//! identically on both sides — Task 1's `unsigned long` equivalence is
//! false on LP64 hosts), the full generation surface, and the fixed seed
//! corpus (8 fast seeds; the full 200-seed corpus runs under `--ignored`).

use std::collections::HashMap;

use fuzz::{generate, run_differential, Input, Program, TYPEDEF_PROLOGUE};

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

// ---------------------------------------------------------------------------
// Task 2: the explicit-width typedef discipline
// ---------------------------------------------------------------------------

#[test]
fn generator_emits_explicit_width_types() {
    // The generated C must use genuinely explicit-width types: msp430-
    // guarded typedefs (u32 = unsigned long on msp430, unsigned int on the
    // host), never bare `unsigned long` globals (64-bit on LP64 hosts).
    let a = generate(0);
    assert!(a.c_source.contains("#ifdef __MSP430__"), "msp430-guarded typedefs");
    assert!(a.c_source.contains("typedef unsigned char u8;"), "u8 typedef");
    assert!(a.c_source.contains("typedef unsigned short u16;"), "u16 typedef");
    assert!(
        a.c_source.contains("typedef unsigned long u32;")
            && a.c_source.contains("typedef unsigned int u32;"),
        "u32: unsigned long on msp430, unsigned int on the host"
    );
    assert!(
        !a.c_source.contains("volatile unsigned long"),
        "no bare unsigned long globals (host unsigned long is 64-bit)"
    );
}

#[test]
fn u32_arithmetic_wraps_identically_on_both_sides() {
    // A u32 value > 2^16 must behave identically on both sides. Task 1's
    // documented equivalence (`unsigned long` is 32-bit on both) is FALSE:
    // on LP64 hosts `unsigned long` is 64-bit, so `x * x` for
    // x = 0xFFFFFFFF is 0xFFFFFFFE00000001 and `x * x > 0xFFFFFFFFu` is 1,
    // while msp430 wraps the mul to 1 and compares 0. The typedef
    // discipline (u32 = unsigned long on msp430, unsigned int on the host)
    // makes u32 arithmetic genuinely 32-bit on both sides, so the checksum
    // agrees with the hand-computed 32-bit semantics: fold(x) = 0,
    // x*x wraps to 1, 1 > 0xFFFFFFFFu is 0 -> checksum = 0.
    let prog = Program {
        c_source: format!(
            "{TYPEDEF_PROLOGUE}\n\
             volatile u32 in0;\n\
             volatile u8 checksum;\n\
             void main(void) {{\n\
               u32 x = in0;\n\
               u8 w = (u8)(x * x > 0xFFFFFFFFu);\n\
               checksum = (u8)((u16)checksum * 7u + (u16)((u8)x ^ (u8)(x >> 8u) ^ (u8)(x >> 16u) ^ (u8)(x >> 24u)));\n\
               checksum = (u8)((u16)checksum * 7u + (u16)w);\n\
             }}\n"
        ),
        inputs: vec![Input {
            name: "in0".into(),
            value: 0xFFFF_FFFF,
            width: 32,
        }],
        checksum_name: "checksum".into(),
    };
    let got = run_differential(&prog)
        .unwrap_or_else(|e| panic!("u32 program not differential-clean: {e}"));
    assert_eq!(got, 0, "u32 wraps at 2^32 identically on both sides");
}

#[test]
fn unsigned_long_u32_arithmetic_mismatches() {
    // Documents WHY the typedef discipline exists: the same program written
    // with `unsigned long` (Task 1's documented width) is NOT
    // differential-clean once u32 values exceed 2^16 — the host computes
    // `x * x` in 64 bits (0xFFFFFFFE00000001 > 0xFFFFFFFFu -> 1), msp430 in
    // 32 (wraps to 1, 1 > 0xFFFFFFFFu -> 0). The harness MUST report it.
    let prog = Program {
        c_source: "volatile unsigned long in0;\n\
                   volatile unsigned char checksum;\n\
                   void main(void) {\n\
                     unsigned long x = in0;\n\
                     checksum = (unsigned char)(x * x > 0xFFFFFFFFu);\n\
                   }\n"
            .to_string(),
        inputs: vec![Input {
            name: "in0".into(),
            value: 0xFFFF_FFFF,
            width: 32,
        }],
        checksum_name: "checksum".into(),
    };
    match run_differential(&prog) {
        Err(e) => assert!(e.contains("mismatch"), "expected a mismatch, got: {e}"),
        Ok(v) => panic!("expected a mismatch (host unsigned long is 64-bit), got Ok({v})"),
    }
}

// ---------------------------------------------------------------------------
// Task 2: the surface + the fixed corpus
// ---------------------------------------------------------------------------

/// (construct, marker): each must appear in the generated source of at
/// least one fast seed, and (with generous counts) across the full corpus.
const SURFACE: &[(&str, &str)] = &[
    ("scalar add", " + "),
    ("scalar sub", " - "),
    ("scalar mul", " * "),
    ("scalar div", " / "),
    ("scalar rem", " % "),
    ("scalar and", " & "),
    ("scalar or", " | "),
    ("scalar xor", " ^ "),
    ("shift", " << "),
    ("comparison", " < "),
    ("if/else", "else"),
    ("bounded loop", "for ("),
    ("noinline call", "helper"),
    ("array", "arr["),
    ("struct", "s."),
];

#[test]
fn fast_corpus_spans_the_generation_surface() {
    // The 8 fixed fast seeds must jointly exercise every construct
    // (deterministic — the RNG stream is fixed, so this never flaps).
    let srcs: Vec<String> = (0..8u64).map(|s| generate(s).c_source).collect();
    for (what, marker) in SURFACE {
        assert!(
            srcs.iter().any(|s| s.contains(marker)),
            "fast corpus (seeds 0..8) does not exercise {what} (marker {marker:?})"
        );
    }
}

#[test]
#[ignore = "full 200-seed corpus (slow)"]
fn full_corpus_differential_clean() {
    for seed in 0..200u64 {
        let prog = generate(seed);
        run_differential(&prog)
            .unwrap_or_else(|e| panic!("generated seed {seed} not differential-clean: {e}"));
    }
}

#[test]
#[ignore = "full 200-seed corpus coverage sanity (slow)"]
fn full_corpus_spans_the_generation_surface() {
    // The committed 200-seed corpus must genuinely span the surface: every
    // construct exercised by a healthy share of seeds (the generator's
    // per-seed feature flags make each appear in most programs; the check
    // pins that the corpus as committed is not degenerate).
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for seed in 0..200u64 {
        let src = generate(seed).c_source;
        for (what, marker) in SURFACE {
            if src.contains(marker) {
                *counts.entry(what).or_insert(0) += 1;
            }
        }
    }
    for (what, marker) in SURFACE {
        let n = counts.get(what).copied().unwrap_or(0);
        assert!(
            n >= 40,
            "{what} (marker {marker:?}) appears in only {n}/200 corpus seeds"
        );
    }
}

#[test]
fn generate_is_deterministic_and_disciplined() {
    let a = generate(42);
    let b = generate(42);
    assert_eq!(a.c_source, b.c_source, "same seed must give the same source");
    assert!(a.c_source.contains("volatile u8 in0;"), "volatile u8 input decl");
    assert!(a.c_source.contains("checksum = (u8)(checksum ^ "), "the checksum fold");
    assert!(a.c_source.ends_with("}\n"), "the generated main ends the file");
    assert_eq!(a.checksum_name, "checksum");
    assert!(!a.inputs.is_empty());
    assert!(
        a.c_source.contains("#ifdef __MSP430__"),
        "the typedef prologue is emitted"
    );
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
