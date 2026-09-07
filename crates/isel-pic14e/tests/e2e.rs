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

/// Like `compile`, but also returns the emitted `.asm` text so a test can
/// inspect the addressing mode (linear vs banked) chosen for an object.
fn compile_asm(c_path: &str) -> (Pic14e, HashMap<String, u16>, String) {
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

    (Pic14e::with_device(&PIC16F1937, words), layout.globals, asm)
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

// P3 end-to-end acceptance (docs/33 section 4): pointers, arrays and
// structs via FSR0/1, plus linear addressing (D-2) for objects that
// straddle a bank. The fixtures are byte-identical to the PIC18 ones of
// the same C source (crates/isel-pic18/tests/fixtures/), so the expected
// values come from the PIC18 e2e tests; `ptr_probe.c` is the RAM-only
// variant (const-flash reads are P4).

#[test]
fn ptr_probe_c_runs_correctly() {
    // in = 0x0035; i = 0x35 & 7 = 5; ram[5] = 0x35; out = ram[5] = 0x35.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_probe.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 0x35; // in low byte
    p.ram_mut()[in_addr + 1] = 0x00; // in high byte
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x35,
        "out == in's low byte read back through the pointer"
    );
    assert!(p.halted());
}

#[test]
fn array_c_runs_correctly() {
    // in low byte = 3 (high byte stays 0) -> buf[3] = 4 -> out = buf[3] = 4.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/array.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 3; // in low byte = 3 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 4, "out == buf[3] == 3+1");
    assert!(p.halted());
}

#[test]
fn structs_c_runs_correctly() {
    // No input seeding, every value is a fixed constant, so out == 0x4E
    // (hand trace in the fixture's comment).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/structs.c"
    ));
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x4E,
        "out == hand-computed 0x4E"
    );
    assert!(p.halted());
}

#[test]
fn banked_ptr_c_runs_correctly() {
    // in low byte = 3 (high byte stays 0) -> out == 0xB8 (hand trace in
    // the fixture's comment).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/banked_ptr.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 3; // in low byte = 3 (high byte stays 0)
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0xB8,
        "out == hand-computed 0xB8 for in == 3"
    );
    assert!(p.halted());
}

/// P3's linear-addressing acceptance (docs/33 section 4): a bank-straddling
/// array must be addressed through the linear region (FSR base in
/// 0x2000-0x29AF) so one FSR walks across banks, while a single-bank array
/// is addressed through the physical (banked) FSR base. The `.asm` is
/// inspected directly, not just the simulated output, so a coincidentally
/// correct byte cannot pass.
#[test]
fn span_c_uses_linear_for_straddling_and_banked_otherwise() {
    let (mut p, globals, asm) = compile_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/span.c"
    ));
    // The spanning array `big` straddles bank 0 -> bank 1, so its FSR base
    // must be the linear alias (0x2000 + bank*80 + (off-0x20)); the
    // single-bank `small` must use its physical base (0xAC, bank 1).
    let big = globals["big"];
    let small = globals["small"];
    // `big` starts in bank 0's GPR and its 90 bytes do not fit the bank-0
    // GPR window (0x20-0x6F), so it straddles into bank 1 (the common-RAM
    // hole 0x70-0x7F is skipped).
    assert!(
        big >= 0x20 && big <= 0x6F && 0x6F - big + 1 < 90,
        "span.c layout: big[90] at 0x{big:03X} must straddle bank 0 -> bank 1"
    );
    assert!(
        small >= 0xA0 && small + 8 <= 0xEF,
        "span.c layout: small[8] at 0x{small:03X} must fit bank 1"
    );
    // Linear base for `big`: 0x2000 + bank0*80 + (0x22 - 0x20) = 0x2002.
    let linear_base = 0x2000 + (big & 0x7F) - 0x20;
    assert!(
        (0x2000..=0x29AF).contains(&linear_base),
        "big's linear base 0x{linear_base:04X} must be in the linear region"
    );
    // The asm must load FSR0H with 0x20 (the linear region's high byte) for
    // the spanning accesses, and with 0x00 for the banked `small` accesses.
    // Simpler, robust check: the asm must contain a MOVLW 0x20 immediately
    // before a MOVWF FSR0H (linear high byte) AND a MOVLW 0x00 before a
    // MOVWF FSR0H (banked high byte), and the linear base 0x2002 must be
    // loaded (MOVLW 0x02; MOVWF FSR0L) for the spanning accesses.
    let lines: Vec<&str> = asm.lines().map(|l| l.trim()).collect();
    let mut linear_hi = false;
    let mut banked_hi = false;
    let mut linear_lo = false;
    for (i, l) in lines.iter().enumerate() {
        if *l == "MOVWF FSR0H" {
            if let Some(prev) = lines.get(i.wrapping_sub(1)) {
                if *prev == "MOVLW 0x20" {
                    linear_hi = true;
                } else if *prev == "MOVLW 0x00" {
                    banked_hi = true;
                }
            }
        }
        if *l == "MOVWF FSR0L" {
            if let Some(prev) = lines.get(i.wrapping_sub(1)) {
                if *prev == "MOVLW 0x02" {
                    linear_lo = true;
                }
            }
        }
    }
    assert!(
        linear_hi && linear_lo,
        "span.c must address the straddling array through the linear region \
         (FSR0 = 0x2002):\n{asm}"
    );
    assert!(
        banked_hi,
        "span.c must address the single-bank array through the physical \
         (banked) FSR base:\n{asm}"
    );
    // And the simulated result must be right: in = 3 -> 0x11 + 0x22 + 0x33.
    p.ram_mut()[globals["in"] as usize] = 3;
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x66,
        "out == 0x11 + 0x22 + 0x33 for in == 3"
    );
    assert!(p.halted());
}
