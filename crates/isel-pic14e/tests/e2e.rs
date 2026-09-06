//! P2 end-to-end acceptance: the same four C programs the PIC14 backend
//! uses (`add.c`, `scalar.c`, `overlay.c`, `banked.c`), compiled through the
//! real PIC14E pipeline (clang -> irparse -> wholeprog -> legalize ->
//! callgraph -> alloc -> isel-pic14e -> banking -> peephole -> asm) and
//! executed in the real `Pic14e` simulator. Mirrors the driver's PIC14E
//! pipeline (docs/33 section 4 P2): the integer spine's 14-bit instructions
//! are bit-identical to classic PIC14, so `isel-pic14e` is isel's emitter,
//! and the divergence is banking, which banks the same physical addresses
//! via `MOVLB`/`BSR` on this core.
//!
//! Fixtures are byte-identical to the PIC14 ones, so the hand-computed
//! expected values come from the PIC14 e2e tests of the same C source.

use asm::assemble_words;
use banking::assign_banks;
use device::PIC16F1937;
use isel_pic14e::{select, select_with_locs};
use peephole::optimize;
use pic14_sim::Pic14e;
use schedule::schedule;
use std::collections::HashMap;
use std::process::Command;
static E2E_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run clang + the full IR pipeline on `c_path`, targeting PIC16F1937
/// through the real PIC14E pipeline, and return a freshly constructed (not
/// yet run) `Pic14e` plus the global address map so each test can seed
/// input addresses by name before calling `.run()`.
fn compile(c_path: &str) -> (Pic14e, HashMap<String, u16>) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let (ll, _dep) = clang_compile(&clang, &resdir, c_path);
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC16F1937.stack_depth as usize);
    let layout = alloc::allocate(&PIC16F1937, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&PIC16F1937, &m, &addrs);
    let asm = schedule(&PIC16F1937, &asm);
    let asm = assign_banks(&PIC16F1937, &asm);
    let asm = optimize(&asm);
    isel_pic14e::verify_page_fit(&m, &asm);
    let words = assemble_words(&PIC16F1937, &asm);

    (Pic14e::with_device(&PIC16F1937, words), layout.globals)
}

/// Run clang alone on `c_path`, returning the `.ll` text and the
/// dependency-file text (the driver's header-detect reads the latter; the
/// P2 fixtures include no headers, so it is unused here but kept for
/// symmetry with the driver pipeline).
fn clang_compile(clang: &str, resdir: &str, c_path: &str) -> (String, String) {
    let out = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-gline-tables-only",
            "-resource-dir",
            resdir,
            "-o",
            "-",
            c_path,
        ])
        .output()
        .expect("run clang");
    assert!(
        out.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (String::from_utf8(out.stdout).unwrap(), String::new())
}

#[test]
fn add_c_runs_correctly() {
    // in = 5 -> out = in + 1 = 6.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/add.c"));
    p.ram_mut()[globals["in"] as usize] = 5;
    p.run(200);
    assert_eq!(p.ram()[globals["out"] as usize], 6);
    assert!(p.halted());
}

#[test]
fn scalar_c_runs_correctly() {
    // Hand trace in the fixture's own comment: in = 7 -> n = 7 -> out = 174.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/scalar.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 7;
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        174,
        "out == hand-computed 174 for in == 7"
    );
    assert!(p.halted());
}

#[test]
fn overlay_c_runs_correctly() {
    // big_a(0) = 28; big_b(0+1) = 8; out = 28 + 8 = 36.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/overlay.c"
    ));
    p.run(500_000);
    assert_eq!(p.ram()[globals["out"] as usize], 36);
    assert!(p.halted());
}

#[test]
fn banked_c_runs_correctly() {
    // Sum 1..90 = 4095 -> out low byte 0xFF.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/banked.c"
    ));
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0xFF);
    assert!(p.halted());
}

/// P2's skip-idiom acceptance (docs/33 section 4): a single `MOVLB` must
/// not land inside a skip pair. The routine-recipe loops in `banked.c`
/// (the DECFSZ-driven mul/div recipes) are skip-sensitive: banking must
/// place the `MOVLB` to switch to an operand's bank *before* the skip
/// test, never between the skip op and its skipped target. Scan the final
/// banked text: no `MOVLB` may directly follow a banked operand that the
/// recursion/loop needs, and no `MOVLB` may sit immediately after a skip
/// op. Concretely, assert the emitted asm contains `MOVLB` (banking did
/// run) and that no line between a skip op and its target begins with
/// `MOVLB`.
#[test]
fn skip_idioms_never_land_a_movlb_between_skip_and_target() {
    let _guard = E2E_LOCK.lock();
    let ll = {
        let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
        let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
        let (ll, _) = clang_compile(
            &clang,
            &resdir,
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/banked.c"),
        );
        ll
    };
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&PIC16F1937, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let (asm, locs) = select_with_locs(&PIC16F1937, &m, &addrs);
    let (asm, _) = schedule::schedule_with_locs(&PIC16F1937, &asm, &locs);
    let (asm, _) = banking::assign_banks_with_locs(&PIC16F1937, &asm, &[]);
    let asm = optimize(&asm);
    assert!(
        asm.lines().any(|l| l.trim().starts_with("MOVLB")),
        "banked.c must exercise BSR-banked addressing on PIC14E (found no MOVLB)"
    );
    // Walk line-by-line; a skip op (BTFSC/BTFSS/INCFSZ/DECFSZ) guards the
    // next instruction. No MOVLB may be that next instruction (a bank
    // switch there would be skipped, leaving the following banked operand
    // in the wrong bank).
    let lines: Vec<&str> = asm.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let mne = line.trim().split_whitespace().next().unwrap_or("");
        if banking::SKIP_OPS.contains(&mne) {
            if let Some(next) = lines.get(i + 1) {
                assert!(
                    !next.trim().starts_with("MOVLB"),
                    "skip pair must not be broken by a MOVLB:\n  {}:\n    {}\n    {}",
                    mne,
                    line.trim(),
                    next.trim()
                );
            }
        }
    }
}
