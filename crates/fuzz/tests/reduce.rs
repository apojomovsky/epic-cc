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

use std::path::PathBuf;

use fuzz::{
    generate, reduce, run_differential, write_fixture, FailureKind, Input, Program,
    REDUCTION_CAP, TYPEDEF_PROLOGUE,
};

/// Removes a fixture file on drop. Synthetic fixtures must not survive the
/// test — on success OR on failure: the old code removed the file only on
/// the success path, leaving `reduced_<seed>.c` behind when an assert
/// panicked mid-test.
struct FixtureGuard(PathBuf);

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

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
            is_float: false,
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
    // saved file is exactly the reduced source. The FixtureGuard removes it
    // on every exit path (success or a panicking assert) — a synthetic
    // fixture (not a real bug) must never survive the test.
    let path = write_fixture(&reduced.program).expect("save the reduced fixture");
    let _guard = FixtureGuard(path.clone());
    assert!(
        path.ends_with(&format!("fixtures/reduced_{SYNTHETIC_SEED}.c")),
        "fixture path: {}",
        path.display()
    );
    let saved = std::fs::read_to_string(&path).expect("read the saved fixture back");
    assert_eq!(saved, reduced.program.c_source, "the fixture is the reduced program");
    drop(_guard);
    assert!(
        !path.exists(),
        "the synthetic fixture must not survive the test (success path)"
    );
}

/// A second mismatch program whose failure MESSAGE differs from the
/// synthetic one — `(int)in0 * 300 < 40000` with in0 = 200: on the PIC the
/// product wraps to -5536 (negative, so the comparison yields 1); on the
/// host it stays 60000 (> 40000, so 0) — `mismatch: pic 1, host 0` vs the
/// synthetic's `mismatch: pic 0, host 1`. (`< 0` would NOT mismatch: clang
/// folds it to 0 under the signed-overflow UB assumption — the comparison
/// must be one the optimizer cannot constant-fold.)
fn other_mismatch_program() -> Program {
    let c_source = "volatile unsigned char in0;\n\
                   volatile unsigned char checksum;\n\
                   void main(void){ checksum = (unsigned char)((int)in0 * 300 < 40000); }\n"
        .to_string();
    Program {
        c_source: c_source.clone(),
        inputs: vec![Input {
            name: "in0".into(),
            value: 200,
            width: 8,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: 9996,
        statements: Vec::new(),
        prologue: c_source,
    }
}

#[test]
fn reduce_captures_the_fresh_failure_message() {
    // reduce() re-derives the failure kind from a fresh verification run of
    // `program`; it must take the MESSAGE from that same run too — a stale
    // caller failure (same kind, a different program's message) must not
    // leak its message into the reduced failure.
    let prog = synthetic_mismatch_program();
    let fresh = match run_differential(&prog) {
        Err(f) => f,
        Ok(v) => panic!("the synthetic program must fail the differential, got Ok({v})"),
    };
    let stale = match run_differential(&other_mismatch_program()) {
        Err(f) => f,
        Ok(v) => panic!("the other program must fail the differential, got Ok({v})"),
    };
    assert_eq!(stale.kind, fresh.kind, "both failures are mismatches");
    assert_ne!(
        stale.to_string(),
        fresh.to_string(),
        "the two failure messages must differ (otherwise the stale one cannot be detected)"
    );

    let reduced = reduce(&prog, &stale).expect("reduce the synthetic mismatch");
    assert_eq!(
        reduced.failure,
        fresh,
        "the reduced failure must be the FRESH verification failure, not the caller's stale one"
    );
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

// ---------------------------------------------------------------------------
// Task 4: the panic-catching demonstration (the loud-panic contract)
// ---------------------------------------------------------------------------

/// The panic program's fixture seed (a marker — the program is hand-written;
/// the seed only names the fixture file).
const PANIC_SEED: u64 = 9997;

/// The unsupported surface: an `i64` (`long long`) computation. msp430 clang
/// emits 64-bit types/ops, and irparse panics on the i64 ("SPIKE LIMIT:
/// unsupported type \"i64\"") — the loud-panic contract for a construct the
/// whole-program compiler does not support.
///
/// NOTE (updated for Milestone 15): the `float` surface WAS the panic
/// culprit through Milestone 14 — Tasks 1-3 implement it (Ty::F32, the
/// lowering, and the soft-float runtime routines), so a float program is
/// now differential-clean and the panic demonstration moved to `i64`, the
/// last unsupported type. The signed-op note below is still true (the
/// signed runtime routines/predicates are implemented, so a signed program
/// is differential-clean too).
const PANIC_CULPRIT: &str = "volatile long long x = (long long)in2;";

/// A synthetic panic program: two benign (differential-clean) statements +
/// an i64 computation the PIC pipeline cannot parse. The benign statements
/// are independent of the culprit, so the reducer can peel them.
fn synthetic_panic_program() -> Program {
    let prologue = format!(
        "{TYPEDEF_PROLOGUE}\n\
         volatile u32 in2;\n\
         volatile u8 checksum;\n\
         void main(void) {{\n"
    );
    let statements = vec![
        "  u8 t0 = (u8)((u8)in2 ^ (u8)(in2 >> 16u));".to_string(),
        "  checksum = (u8)(checksum ^ (u8)t0);".to_string(),
        PANIC_CULPRIT.to_string(),
        "  checksum = (u8)(checksum ^ (u8)(x >> 32));".to_string(),
    ];
    let c_source = format!("{prologue}{}\n}}\n", statements.join("\n"));
    Program {
        c_source,
        prologue,
        statements,
        inputs: vec![Input {
            name: "in2".into(),
            value: 0x1234_5678,
            width: 32,
            is_float: false,
        }],
        checksum_name: "checksum".into(),
        seed: PANIC_SEED,
    }
}

#[test]
fn differential_reports_unsupported_construct_panic_and_reducer_minimizes() {
    // The loud-panic contract end to end: a program with an unsupported
    // construct must FAIL the differential (kind Panic — the PIC pipeline
    // panic is caught, not propagated), and the reducer must minimize it
    // while preserving the panic.
    let prog = synthetic_panic_program();
    let failure = match run_differential(&prog) {
        Err(f) => f,
        Ok(v) => panic!("the i64 program must panic the PIC pipeline, got Ok({v})"),
    };
    assert_eq!(
        failure.kind,
        FailureKind::Panic,
        "an unsupported construct must be a Panic failure: {failure}"
    );
    assert!(
        failure.to_string().contains("i64"),
        "the panic message should name the unsupported i64 op: {failure}"
    );

    let reduced = reduce(&prog, &failure).expect("reduce the panic");
    assert!(reduced.re_runs <= REDUCTION_CAP, "the reduction must respect the cap");
    match run_differential(&reduced.program) {
        Err(f) => assert_eq!(
            f.kind,
            FailureKind::Panic,
            "the reduced program must still panic: {f}"
        ),
        Ok(v) => panic!("the reduced program must still panic, got Ok({v})"),
    }
    assert!(
        reduced.program.c_source.contains("long long"),
        "the reduced program must keep the unsupported construct:\n{}",
        reduced.program.c_source
    );
    assert!(
        reduced.program.statements.len() < prog.statements.len(),
        "the reduction must shrink the program: {} -> {} statements",
        prog.statements.len(),
        reduced.program.statements.len()
    );

    // The reduced program is saved as the reduced_<seed>.c fixture (the
    // panic-catching evidence) and the guard removes it on every path — the
    // i64 surface is a documented limitation, not a NEW bug to commit.
    let path = write_fixture(&reduced.program).expect("save the reduced fixture");
    let _guard = FixtureGuard(path.clone());
    let saved = std::fs::read_to_string(&path).expect("read the saved fixture back");
    assert_eq!(saved, reduced.program.c_source, "the fixture is the reduced program");
    drop(_guard);
    assert!(
        !path.exists(),
        "the synthetic panic fixture must not survive the test (success path)"
    );
}
