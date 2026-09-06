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

/// P2's skip-idiom acceptance (docs/33 section 4): a routine-recipe-shaped
/// program whose `__mul_u16` DECFSZ loops are skip-sensitive, compiled for
/// PIC14E, must neither straddle the routine's bank-switch demand across a
/// skip pair nor land a `MOVLB` right after a skip op. `banked_routine.c`
/// (issue #6): its noinline `mul30` pushes `__mul_u16`'s frame into a
/// non-zero bank, so the banking pass emits `MOVLB`s around the recipe
/// call; the DECFSZ loops inside the recipe body are the skip-window
/// hazard docs/33 section 1 names. Assert both (a) the emitted asm DOES
/// contain `MOVLB` (banking ran for real) and (b) no `MOVLB` is the
/// immediately-following instruction of any skip op.
#[test]
fn skip_idioms_never_land_a_movlb_inside_a_recipe_skip_pair() {
    let _guard = E2E_LOCK.lock();
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let (ll, _) = clang_compile(
        &clang,
        &resdir,
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/banked_routine.c"
        ),
    );
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
        "banked_routine.c must exercise BSR-banked addressing on PIC14E (found no MOVLB)",
    );
    // The single-GPR-bank routine-frame constraint (issue #6): `__mul_u16`'s
    // whole frame lives in ONE bank, so no MOVLB belongs inside its
    // skip-sensitive DECFSZ body. Walk every skip op's immediately-following
    // instruction: a MOVLB there would be the skip's target and, when the
    // skip is taken (loop-exit), would be skipped, leaving the subsequent
    // banked operand in the wrong bank.
    let lines: Vec<&str> = asm.lines().collect();
    let mut split_pairs = 0;
    for (i, line) in lines.iter().enumerate() {
        let mne = line.trim().split_whitespace().next().unwrap_or("");
        if banking::SKIP_OPS.contains(&mne) {
            if let Some(next) = lines.get(i + 1) {
                if next.trim().starts_with("MOVLB") {
                    split_pairs += 1;
                }
            }
        }
    }
    assert_eq!(
        split_pairs, 0,
        "a MOVLB landed inside a skip pair; the recipe frame straddles banks:\n{asm}"
    );
    // The single-GPR-bank frame constraint (issue #6, docs/33 section 1):
    // assert the whole `__mul_u16` frame (params + scratch) lies inside ONE
    // bank, so no MOVLB was ever needed inside its skip-sensitive body.
    let scr = *layout
        .locals
        .get("__mul_u16::__scr")
        .expect("__mul_u16::__scr");
    let a = *layout.locals.get("__mul_u16::a").expect("__mul_u16::a");
    let b = *layout.locals.get("__mul_u16::b").expect("__mul_u16::b");
    let all = [a, b, scr];
    let bank_idx = PIC16F1937
        .ram_banks
        .iter()
        .position(|&(s, e)| a >= s && a <= e)
        .expect("__mul_u16 params must land in a GPR bank");
    let (bs, be) = PIC16F1937.ram_banks[bank_idx];
    for &x in &all {
        assert!(
            x >= bs && x <= be,
            "recipe frame byte 0x{x:03X} is outside bank {bank_idx} (0x{bs:03X}-0x{be:03X}); \
             the recipe loops are skip-sensitive and a MOVLB would change skip targets"
        );
    }
}

#[test]
fn banked_routine_c_runs_correctly() {
    // out = (465 * 7) & 0xFFFF = 3255 = 0x0CB7 (sum 1..30 = 465).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/banked_routine.c"
    ));
    p.run(2_000_000);
    let out_addr = globals["out"] as usize;
    assert_eq!(p.ram()[out_addr], 0xB7, "out low byte: 3255 = 0x0CB7");
    assert_eq!(p.ram()[out_addr + 1], 0x0C, "out high byte: 3255 = 0x0CB7");
    assert!(p.halted());
}
