//! Reducer tests (Task 3): the greedy cvise-style reducer must peel the
//! statements that do NOT cause a differential failure and keep the culprit
//! — re-running the differential per candidate deletion and keeping only
//! failure-preserving ones — and the reduced program must be saved as a
//! `reduced_<seed>.c` fixture.
//!
//! The failure under test is a synthetic one: a C-discipline violation (an
//! `int` expression — 16-bit on msp430, 32-bit on the host — so the PIC
//! wraps `(int)in0 * 300` and the comparison flips). The benign statements
//! are differential-clean; the culprit is the only statement that makes the
//! checksums diverge. The reducer must delete the benign statements, keep
//! the culprit, and the reduced program must still fail the differential
//! with the SAME failure kind.

use fuzz::{
    generate, reduce, run_differential, write_fixture, FailureKind, Input, Program,
    REDUCTION_CAP, TYPEDEF_PROLOGUE,
};

/// The synthetic program's fixture seed (a marker — the program is
/// hand-written, not generated; the seed only names the fixture file).
const SYNTHETIC_SEED: u64 = 9999;

/// The culprit marker: a discipline violation whose PIC/host semantics
/// differ (same construction as `mismatching_variant_fails` in
/// tests/differential.rs — `int` is 16-bit on msp430, 32-bit on the host,
/// so `(int)in0 * 300` wraps to -5536 on the PIC and the comparison yields
/// 0, while the host keeps 60000 > 40000 == 1).
const CULPRIT: &str = "(int)in0 * 300 > 40000";

/// A synthetic mismatching program: two benign (differential-clean)
/// statements + one culprit statement that flips the checksum. The benign
/// statements are independent of each other (each references only `in0`),
/// so deleting either is a valid program.
fn synthetic_mismatch_program() -> Program {
    let prologue = format!(
        "{TYPEDEF_PROLOGUE}\n\
         volatile u8 in0;\n\
         volatile u8 checksum;\n\
         void main(void) {{\n"
    );
    let statements = vec![
        "  u8 t0 = (u8)((u8)in0 + 1u);".to_string(),
        "  checksum = (u8)(checksum ^ (u8)t0);".to_string(),
        "  u8 t1 = (u8)((u8)in0 * 3u);".to_string(),
        "  checksum = (u8)(checksum ^ (u8)t1);".to_string(),
        format!("  checksum = (u8)(checksum ^ (u8)({CULPRIT}));"),
    ];
    let c_source = format!("{prologue}{}\n}}\n", statements.join("\n"));
    Program {
        c_source,
        prologue,
        statements,
        inputs: vec![Input {
            name: "in0".into(),
            value: 200,
            width: 8,
        }],
        checksum_name: "checksum".into(),
        seed: SYNTHETIC_SEED,
    }
}

#[test]
fn reducer_removes_benign_statements_and_keeps_the_culprit() {
    let prog = synthetic_mismatch_program();
    let failure = match run_differential(&prog) {
        Err(f) => f,
        Ok(v) => panic!("the synthetic program must fail the differential, got Ok({v})"),
    };
    assert_eq!(failure.kind, FailureKind::Mismatch, "the synthetic failure must be a mismatch");

    let reduced = reduce(&prog, &failure).expect("reduce the synthetic mismatch");
    assert!(reduced.re_runs <= REDUCTION_CAP, "the reduction must respect the cap");

    // The reduced program still fails the differential, with the SAME kind.
    match run_differential(&reduced.program) {
        Err(f) => assert_eq!(
            f.kind,
            FailureKind::Mismatch,
            "the reduced program must still fail with the original failure kind: {f}"
        ),
        Ok(v) => panic!("the reduced program must still fail the differential, got Ok({v})"),
    }

    // The culprit is kept (deleting it makes the differential clean)…
    assert!(
        reduced.program.c_source.contains(CULPRIT),
        "the reduced program must keep the culprit:\n{}",
        reduced.program.c_source
    );
    // …and the benign statements are gone (deleting them preserves the
    // failure, so the greedy loop removed them).
    assert!(
        !reduced.program.c_source.contains("+ 1u"),
        "the benign t0 statement must be removed:\n{}",
        reduced.program.c_source
    );
    assert!(
        !reduced.program.c_source.contains("* 3u"),
        "the benign t1 statement must be removed:\n{}",
        reduced.program.c_source
    );
    assert!(
        reduced.program.statements.len() < prog.statements.len(),
        "the reduction must shrink the program: {} -> {} statements",
        prog.statements.len(),
        reduced.program.statements.len()
    );
    assert_eq!(
        reduced.program.statements.len(),
        1,
        "only the culprit should remain:\n{}",
        reduced.program.c_source
    );
    assert_eq!(
        reduced.statements_removed,
        prog.statements.len() - reduced.program.statements.len(),
        "removed/kept accounting"
    );

    // The reduced program is saved as the reduced_<seed>.c fixture and the
    // saved file is exactly the reduced source.
    let path = write_fixture(&reduced.program).expect("save the reduced fixture");
    assert!(
        path.ends_with(&format!("fixtures/reduced_{SYNTHETIC_SEED}.c")),
        "fixture path: {}",
        path.display()
    );
    let saved = std::fs::read_to_string(&path).expect("read the saved fixture back");
    assert_eq!(saved, reduced.program.c_source, "the fixture is the reduced program");
    std::fs::remove_file(&path).expect("remove the synthetic fixture (not a real bug)");
}

#[test]
#[ignore = "slower: end-to-end reduction of a generated program (manual)"]
fn reducer_minimizes_a_generated_program_with_a_planted_bug() {
    // A real generated program with the synthetic discipline violation
    // planted at the end of main: the reducer must peel the generated
    // statements (the generator's own body, with its local references)
    // down to the planted culprit while preserving the mismatch.
    let mut prog = generate(1);
    let culprit = format!("  checksum = (u8)(checksum ^ (u8)({CULPRIT}));");
    prog.statements.push(culprit);
    prog.c_source = format!("{}{}\n}}\n", prog.prologue, prog.statements.join("\n"));
    prog.inputs[0].value = 200; // in0 = 200 (the value that flips the comparison)
    prog.seed = 9998;

    let failure = match run_differential(&prog) {
        Err(f) => f,
        Ok(v) => panic!("the planted program must fail the differential, got Ok({v})"),
    };
    assert_eq!(failure.kind, FailureKind::Mismatch);

    let reduced = reduce(&prog, &failure).expect("reduce the planted generated program");
    assert!(reduced.re_runs <= REDUCTION_CAP, "the reduction must respect the cap");
    match run_differential(&reduced.program) {
        Err(f) => assert_eq!(f.kind, FailureKind::Mismatch, "failure preserved: {f}"),
        Ok(v) => panic!("the reduced program must still fail, got Ok({v})"),
    }
    assert!(
        reduced.program.c_source.contains(CULPRIT),
        "the reduced program must keep the planted culprit"
    );
    assert!(
        reduced.program.statements.len() < prog.statements.len(),
        "the reduction must shrink the generated program: {} -> {}",
        prog.statements.len(),
        reduced.program.statements.len()
    );
}
