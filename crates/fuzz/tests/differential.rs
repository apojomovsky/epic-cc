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

use fuzz::{generate, generate_float, run_differential, FailureKind, Input, Program, TYPEDEF_PROLOGUE};

/// The brief's tiny program: one u8 volatile input, one scalar expression.
const TINY: &str = "volatile unsigned char in0;\n\
                    volatile unsigned char checksum;\n\
                    void main(void){ checksum = (unsigned char)(in0 * 7 + 3); }\n";

fn tiny_program(in0: u32) -> Program {
    Program {
        c_source: TINY.to_string(),
        inputs: vec![Input {
            name: "in0".into(),
            value: in0,
            width: 8,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: 0,
        statements: Vec::new(),
        prologue: TINY.to_string(),
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
    let c_source = "volatile unsigned char in0;\n\
                   volatile unsigned char checksum;\n\
                   void main(void){ checksum = (unsigned char)((int)in0 * 300 > 40000); }\n"
        .to_string();
    let prog = Program {
        c_source: c_source.clone(),
        inputs: vec![Input {
            name: "in0".into(),
            value: 200,
            width: 8,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: 0,
        statements: Vec::new(),
        prologue: c_source,
    };
    match run_differential(&prog) {
        Err(e) => assert!(e.to_string().contains("mismatch"), "expected a mismatch, got: {e}"),
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
    let c_source = format!(
        "{TYPEDEF_PROLOGUE}\n\
         volatile u32 in0;\n\
         volatile u8 checksum;\n\
         void main(void) {{\n\
           u32 x = in0;\n\
           u8 w = (u8)(x * x > 0xFFFFFFFFu);\n\
           checksum = (u8)((u16)checksum * 7u + (u16)((u8)x ^ (u8)(x >> 8u) ^ (u8)(x >> 16u) ^ (u8)(x >> 24u)));\n\
           checksum = (u8)((u16)checksum * 7u + (u16)w);\n\
         }}\n"
    );
    let prog = Program {
        c_source: c_source.clone(),
        inputs: vec![Input {
            name: "in0".into(),
            value: 0xFFFF_FFFF,
            width: 32,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: 0,
        statements: Vec::new(),
        prologue: c_source,
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
    let c_source = "volatile unsigned long in0;\n\
                   volatile unsigned char checksum;\n\
                   void main(void) {\n\
                     unsigned long x = in0;\n\
                     checksum = (unsigned char)(x * x > 0xFFFFFFFFu);\n\
                   }\n"
        .to_string();
    let prog = Program {
        c_source: c_source.clone(),
        inputs: vec![Input {
            name: "in0".into(),
            value: 0xFFFF_FFFF,
            width: 32,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: 0,
        statements: Vec::new(),
        prologue: c_source,
    };
    match run_differential(&prog) {
        Err(e) => assert!(
            e.to_string().contains("mismatch"),
            "expected a mismatch (host unsigned long is 64-bit), got: {e}"
        ),
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
    // The acceptance gate: all 200 committed seeds must run
    // differential-clean. The per-kind counts are printed (--nocapture) so
    // the acceptance run documents the outcome distribution; any non-clean
    // seed panics with its classified failure.
    let mut clean = 0usize;
    let mut mismatch = 0usize;
    let mut panic_kind = 0usize;
    let mut nohalt = 0usize;
    let mut compile = 0usize;
    let mut harness = 0usize;
    for seed in 0..200u64 {
        let prog = generate(seed);
        match run_differential(&prog) {
            Ok(_) => clean += 1,
            Err(f) => {
                match f.kind {
                    FailureKind::Mismatch => mismatch += 1,
                    FailureKind::Panic => panic_kind += 1,
                    FailureKind::NoHalt => nohalt += 1,
                    FailureKind::Compile => compile += 1,
                    FailureKind::Harness => harness += 1,
                }
                println!(
                    "corpus failure at seed {seed}: {f} (running: clean {clean}, mismatch \
                     {mismatch}, panic {panic_kind}, nohalt {nohalt}, compile {compile}, \
                     harness {harness})"
                );
                panic!("generated seed {seed} not differential-clean: {f}");
            }
        }
    }
    println!(
        "corpus (200 seeds): clean {clean}, mismatch {mismatch}, panic {panic_kind}, \
         nohalt {nohalt}, compile {compile}, harness {harness}"
    );
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
    println!("surface coverage across the 200-seed corpus:");
    for (what, marker) in SURFACE {
        let n = counts.get(what).copied().unwrap_or(0);
        println!("  {what}: {n}/200 seeds");
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

// ---------------------------------------------------------------------------
// Task 5: the float differential (RNE at scale)
// ---------------------------------------------------------------------------

/// The float-fold program pieces shared by the fixed float tests: the
/// globals (in0 u8, the float inputs in3..in6, the checksum, the bits-fold
/// global `fout`), the fold32 helper, and the fold's shape. Every float
/// result is stored to the volatile `fout` global and re-read as a u32 —
/// `*(volatile u32*)&fout` is a plain `load i32` of the float global's
/// bytes under LLVM's opaque pointers (no bitcast inst, so the PIC
/// pipeline parses it) — and the fold32 byte-mix feeds the checksum. A
/// single wrong RNE bit in any float result changes the fold.
const FLOAT_GLOBALS: &str = "\
volatile u8 in0;\n\
volatile float in3;\n\
volatile float in4;\n\
volatile float in5;\n\
volatile float in6;\n\
volatile u8 checksum;\n\
volatile float fout;\n\
__attribute__((noinline)) u8 fold32(u32 v) {\n\
    return (u8)((u8)v ^ (u8)(v >> 8u) ^ (u8)(v >> 16u) ^ (u8)(v >> 24u) + (u8)in0);\n\
}\n";

/// A fixed float test program: the float-fold globals + `body` in main.
/// The fixed programs are kept to ~2 statements: main's frame holds every
/// SSA def, and it must leave bank-0 room for the soft-float routine slots
/// (the isel's loud assert — main_end + 14 <= 0x70), exactly the budget the
/// generator models. `in3` is parameterized for the 1-ulp sensitivity pin.
fn float_prog(body: &str, in3: u32) -> Program {
    let c_source = format!("{TYPEDEF_PROLOGUE}{FLOAT_GLOBALS}void main(void) {{\n{body}}}\n");
    Program {
        c_source: c_source.clone(),
        inputs: vec![
            Input { name: "in0".into(), value: 0, width: 8, is_float: false },
            Input { name: "in3".into(), value: in3, width: 32, is_float: true },
            Input { name: "in4".into(), value: 0x4000_0000, width: 32, is_float: true }, // 2.0f
            Input { name: "in5".into(), value: 0x4040_0000, width: 32, is_float: true }, // 3.0f
            Input { name: "in6".into(), value: 0x3DCC_CCCD, width: 32, is_float: true }, // 0.1f
        ],
        checksum_name: "checksum".into(),
        seed: 0,
        statements: Vec::new(),
        prologue: c_source,
    }
}

/// The fold statement for a float local: store to `fout`, re-read the bits.
const FOLD: &str = "  fout = %;\n  checksum = (u8)(checksum ^ fold32(*(volatile u32*)&fout));\n";

fn ffold(t: &str) -> String {
    FOLD.replace('%', t)
}

#[test]
fn float_fixed_arith_clean() {
    // fadd + fmul, exact: t0 = 1.0f + 2.0f = 3.0f (0x40400000, fold 00),
    // t1 = 3.0f * 3.0f = 9.0f (0x41100000, fold 00^00^10^41 = 51).
    // checksum = 00 ^ 51 = 51 (hand-computed; the host's SSE RNE is the
    // oracle, the PIC routines must agree bit-for-bit).
    let body = format!(
        "  float t0 = in3 + in4;\n{fold0}  float t1 = t0 * in5;\n{fold1}",
        fold0 = ffold("t0"),
        fold1 = ffold("t1"),
    );
    let got = run_differential(&float_prog(&body, 0x3F80_0000))
        .unwrap_or_else(|e| panic!("float fixed arith not differential-clean: {e}"));
    assert_eq!(got, 0x51, "hand-computed float add+mul checksum");
}

#[test]
fn float_fixed_divsub_clean() {
    // fdiv + fsub, exact: t0 = 3.0f / 1.0f = 3.0f (fold 00),
    // t1 = 3.0f - 2.0f = 1.0f (0x3F800000, fold 00^00^80^3F = BF).
    // checksum = 00 ^ BF = BF.
    let body = format!(
        "  float t0 = in5 / in3;\n{fold0}  float t1 = t0 - in4;\n{fold1}",
        fold0 = ffold("t0"),
        fold1 = ffold("t1"),
    );
    let got = run_differential(&float_prog(&body, 0x3F80_0000))
        .unwrap_or_else(|e| panic!("float fixed div/sub not differential-clean: {e}"));
    assert_eq!(got, 0xBF, "hand-computed float div+sub checksum");
}

#[test]
fn float_fixed_rne_clean() {
    // The milestone's load-bearing RNE cases: t0 = 1.0f + 0.1f = 1.1f
    // (0x3F8CCCCD, fold CD^CC^8C^3F = B2), t1 = 1.1f / 3.0f = 0x3EBBBBBC
    // (fold BC^BB^BB^3E = 82). checksum = B2 ^ 82 = 30. A wrong round bit
    // in either routine changes these bits and the checksum.
    let body = format!(
        "  float t0 = in3 + in6;\n{fold0}  float t1 = t0 / 3.0f;\n{fold1}",
        fold0 = ffold("t0"),
        fold1 = ffold("t1"),
    );
    let got = run_differential(&float_prog(&body, 0x3F80_0000))
        .unwrap_or_else(|e| panic!("float fixed RNE not differential-clean: {e}"));
    assert_eq!(got, 0x30, "hand-computed RNE checksum (1.0+0.1 and 1.1/3)");
}

#[test]
fn float_fixed_cmp_clean() {
    // fcmp: c0 = (1.0f < 2.0f) = 1; c1 = (-0.0f == 0.0f) = 1 — the Task-3
    // cmp fix's zero-equality (in6 = 0x80000000 = -0.0). checksum = 1 ^ 1 =
    // 0; a sign-magnitude bug (e.g. -0 < +0) flips c1 and mismatches.
    let body = "\
  checksum = (u8)(checksum ^ (u8)(in3 < in4));\n\
  checksum = (u8)(checksum ^ (u8)(in6 == 0.0f));\n";
    let mut prog = float_prog(body, 0x3F80_0000);
    prog.inputs[4].value = 0x8000_0000; // in6 = -0.0
    let got = run_differential(&prog)
        .unwrap_or_else(|e| panic!("float fixed fcmp not differential-clean: {e}"));
    assert_eq!(got, 0x00, "-0.0 == +0.0 (and 1.0 < 2.0)");
}

#[test]
fn float_fixed_convs_clean() {
    // uitofp + fptoui: t0 = (float)0x3F800003 = 1065353216.0f (the nearest
    // float — 0x4E7E0000, fold 00^00^7E^4E = 30); fptoui of
    // (float)(3 & 0xFFFF) = 3 (fold 03). checksum = 30 ^ 03 = 33.
    let body = format!(
        "  float t0 = (float)(*(volatile u32*)&in3);\n{fold0}\
         \x20 checksum = (u8)(checksum ^ fold32((u32)((float)((*(volatile u32*)&in3) & 0xFFFFu))));\n",
        fold0 = ffold("t0"),
    );
    let got = run_differential(&float_prog(&body, 0x3F80_0003))
        .unwrap_or_else(|e| panic!("float fixed uitofp/fptoui not differential-clean: {e}"));
    assert_eq!(got, 0x33, "hand-computed uitofp/fptoui checksum");
}

#[test]
fn float_fixed_signed_convs_clean() {
    // sitofp + fptosi: t0 = (float)(s32)0x3F800003 = 0x4E7E0000 (fold 30);
    // fptosi of (float)(s32)(3 & 0xFFFF) = 3 (fold 03). checksum 30 ^ 03.
    let body = format!(
        "  float t0 = (float)(s32)(*(volatile u32*)&in3);\n{fold0}\
         \x20 checksum = (u8)(checksum ^ fold32((u32)((s32)((float)(s32)((*(volatile u32*)&in3) & 0xFFFFu)))));\n",
        fold0 = ffold("t0"),
    );
    let got = run_differential(&float_prog(&body, 0x3F80_0003))
        .unwrap_or_else(|e| panic!("float fixed sitofp/fptosi not differential-clean: {e}"));
    assert_eq!(got, 0x33, "hand-computed sitofp/fptosi checksum");
}

#[test]
fn float_fold_is_ulp_sensitive() {
    // A single wrong RNE bit must change the checksum: with in3 flipped by
    // 1 ulp (0x3F800001 = 1.0f + 2^-23), t0 = in3 + 0.1f rounds to
    // 0x3F8CCCCE (not 0x3F8CCCCD) and t1 = t0 / 3.0f rounds to 0x3EBBBBBD
    // — the fold changes, so the differential would catch a rounding error
    // in ANY statement. 0x32 verified against Rust f32 semantics.
    let body = format!(
        "  float t0 = in3 + in6;\n{fold0}  float t1 = t0 / 3.0f;\n{fold1}",
        fold0 = ffold("t0"),
        fold1 = ffold("t1"),
    );
    let a = run_differential(&float_prog(&body, 0x3F80_0000))
        .unwrap_or_else(|e| panic!("float RNE program (1.0f) not clean: {e}"));
    let b = run_differential(&float_prog(&body, 0x3F80_0001))
        .unwrap_or_else(|e| panic!("float RNE program (1.0f + 1ulp) not clean: {e}"));
    assert_eq!(a, 0x30, "the 1.0f run's hand-computed checksum");
    assert_eq!(b, 0x32, "the 1.0f + 1ulp run's checksum (Rust f32 reference)");
    assert_ne!(a, b, "a 1-ulp input change must change the checksum");
}

/// (construct, marker): each must appear in the float corpus's generated
/// main bodies. Checked on `program.statements` (the main body only), so
/// the fold32 helper's own text cannot false-positive.
const FLOAT_SURFACE: &[(&str, &str)] = &[
    ("fadd", " + "),
    ("fsub", " - "),
    ("fmul", " * "),
    ("fdiv", " / "),
    ("fcmp", " ^ (u8)("),
    ("uitofp", "(float)(*(volatile u32*)&in3)"),
    ("sitofp", "(float)(s32)(*(volatile u32*)&in3)"),
    ("fptoui", "(u32)((float)("),
    ("fptosi", "(s32)((float)("),
    ("the bits fold", "*(volatile u32*)&fout"),
];

#[test]
fn generate_float_is_deterministic_and_spanning() {
    let a = generate_float(42);
    let b = generate_float(42);
    assert_eq!(
        a.c_source, b.c_source,
        "the same float seed must give the same source"
    );
    // The float inputs are volatile float globals (the bit-pattern filter is
    // documented on generate_float: no NaN/inf/denormal inputs). Float
    // RESULTS fold over their BITS through the fout global — every seed
    // with a float-result statement (fbin/uitofp/sitofp) uses it, and the
    // union of the fast seeds covers it (cmp/fptoui-only seeds fold
    // directly).
    let srcs: Vec<String> = (0..8u64).map(|s| generate_float(s).c_source).collect();
    for s in &srcs {
        assert!(s.contains("volatile float in3;"), "float input decl");
        assert!(s.contains("volatile float fout;"), "the bits-fold global");
    }
    assert!(
        srcs.iter().any(|s| s.contains("*(volatile u32*)&fout")),
        "the fold reads the float BITS"
    );
    // The fast span window covers the 6 families by construction (the
    // forced first statement rotates seed % 6 over add/sub/mul/div/cmp/conv)
    // and the conversion SUB-kinds (fptoui/fptosi) via the forced Conv
    // rotation ((seed / 6) % 4 — seeds 5/11/17/23). Source generation only.
    let srcs: Vec<String> = (0..24u64).map(|s| generate_float(s).c_source).collect();
    for &(what, marker) in FLOAT_SURFACE {
        assert!(
            srcs.iter().any(|s| s.contains(marker)),
            "float fast seeds (0..24) do not exercise {what} (marker {marker:?})"
        );
    }
}

#[test]
#[ignore = "full 50-seed float corpus (slow)"]
fn float_corpus_differential_clean() {
    // The acceptance gate: all 50 committed float seeds must run
    // differential-clean. The per-kind counts are printed (--nocapture) so
    // the acceptance run documents the outcome distribution; any non-clean
    // seed panics with its classified failure. This is the milestone's RNE
    // verification at scale: the host's SSE rounding is the oracle for the
    // soft-float routines.
    let mut clean = 0usize;
    let mut mismatch = 0usize;
    let mut panic_kind = 0usize;
    let mut nohalt = 0usize;
    let mut compile = 0usize;
    let mut harness = 0usize;
    for seed in 0..50u64 {
        let prog = generate_float(seed);
        match run_differential(&prog) {
            Ok(_) => clean += 1,
            Err(f) => {
                match f.kind {
                    FailureKind::Mismatch => mismatch += 1,
                    FailureKind::Panic => panic_kind += 1,
                    FailureKind::NoHalt => nohalt += 1,
                    FailureKind::Compile => compile += 1,
                    FailureKind::Harness => harness += 1,
                }
                println!(
                    "float corpus failure at seed {seed}: {f} (running: clean {clean}, \
                     mismatch {mismatch}, panic {panic_kind}, nohalt {nohalt}, compile \
                     {compile}, harness {harness})"
                );
                panic!("generated float seed {seed} not differential-clean: {f}");
            }
        }
    }
    println!(
        "float corpus (50 seeds): clean {clean}, mismatch {mismatch}, panic {panic_kind}, \
         nohalt {nohalt}, compile {compile}, harness {harness}"
    );
}

#[test]
#[ignore = "full 50-seed float corpus coverage sanity (slow)"]
fn float_corpus_spans_the_float_surface() {
    // The committed 50-seed float corpus must genuinely span the float
    // surface: each construct exercised by a healthy share of seeds (the
    // forced rotation alone gives each family ~8 seeds; the conversion
    // sub-kinds come from the forced Conv rotation + the fill).
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for seed in 0..50u64 {
        let prog = generate_float(seed);
        for &(what, marker) in FLOAT_SURFACE {
            if prog.statements.iter().any(|s| s.contains(marker)) {
                *counts.entry(what).or_insert(0) += 1;
            }
        }
    }
    println!("float surface coverage across the 50-seed corpus:");
    for &(what, marker) in FLOAT_SURFACE {
        let n = counts.get(what).copied().unwrap_or(0);
        println!("  {what}: {n}/50 seeds");
        let min = match what {
            "fadd" | "fsub" | "fmul" | "fdiv" | "fcmp" => 8,
            "uitofp" | "sitofp" => 5,
            _ => 3, // fptoui/fptosi: 2 forced + fill
        };
        assert!(
            n >= min,
            "{what} (marker {marker:?}) appears in only {n}/50 float corpus seeds"
        );
    }
}
