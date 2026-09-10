// P2 end-to-end acceptance (docs/37 section 3 P2): the integer spine
// compiled through the real PIC baseline pipeline (clang -> irparse ->
// wholeprog -> legalize -> callgraph -> alloc -> isel-pic-baseline ->
// asm) and executed in the real `PicBaseline` simulator. The fixtures
// exercise basic integer arithmetic (add.c, scalar.c) and D-2's FSR
// bank-bit reassertion end-to-end through real codegen (banked.c), not
// just the hand-written P1 `.asm` test.

use asm::assemble_words;
use isel_pic_baseline::select;
use parking_lot::Mutex;
use pic14_sim::PicBaseline;
use std::collections::HashMap;
use std::process::Command;

static E2E_LOCK: Mutex<()> = Mutex::new(());

/// Run clang + the full IR pipeline on `c_path`, targeting p12f509
/// through the real PIC baseline pipeline, and return a freshly
/// constructed (not yet run) `PicBaseline` plus the global address map so
/// each test can seed input addresses by name before calling `.run()`.
fn compile(c_path: &str) -> (PicBaseline, HashMap<String, u16>) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let (ll, _dep) = clang_compile(&clang, &resdir, c_path);
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, device::PIC12F509.stack_depth as usize);
    let layout = alloc::allocate(&device::PIC12F509, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&device::PIC12F509, &m, &addrs);
    isel_pic_baseline::verify_page_fit(&m, &asm);
    let words = assemble_words(&device::PIC12F509, &asm);

    (
        PicBaseline::with_device(&device::PIC12F509, words),
        layout.globals,
    )
}

/// Like `compile`, but also returns the emitted `.asm` text so a test can
/// inspect the D-2 reassertion instructions.
fn compile_asm(c_path: &str) -> (PicBaseline, HashMap<String, u16>, String) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let (ll, _dep) = clang_compile(&clang, &resdir, c_path);
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, device::PIC12F509.stack_depth as usize);
    let layout = alloc::allocate(&device::PIC12F509, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&device::PIC12F509, &m, &addrs);
    isel_pic_baseline::verify_page_fit(&m, &asm);
    let words = assemble_words(&device::PIC12F509, &asm);

    (
        PicBaseline::with_device(&device::PIC12F509, words),
        layout.globals,
        asm,
    )
}

/// Run clang alone on `c_path`, returning the `.ll` text and the
/// dependency-file text.
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
            "-g",
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
        "clang failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn add_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/add.c");
    p.ram_mut()[globals["in"] as usize] = 5;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 6, "out = in + 1");
    assert!(p.halted());
}

#[test]
fn scalar_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/scalar.c");
    p.ram_mut()[globals["in"] as usize] = 7;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 174, "scalar trace");
    assert!(p.halted());
}

/// D-2's end-to-end acceptance (docs/37 section 3 P2): a fixture with
/// globals in both banks plus a pointer (FSR/INDF) deref, exercising the
/// `BCF`/`BSF FSR,5` reassertion sequence through real codegen. The `.asm`
/// is inspected to assert the reassertion instructions are present before
/// bank-1 direct accesses and before INDF touches.
#[test]
fn banked_c_runs_correctly_and_reasserts_fsr5() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/banked.c");
    p.run(10_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        20,
        "out = deref(&g19) = 20 (g19 is in bank 1)"
    );
    assert!(p.halted());
    // D-2: the emitted asm must contain BSF FSR,5 (bank-1 direct access)
    // and BCF FSR,5 (bank-0 direct access). The INDF touches must NOT be
    // preceded by BCF FSR,5: the pointer load sets FSR<5>, and clearing it
    // would redirect a bank-1 pointer to bank 0.
    assert!(
        asm.contains("BSF FSR, 5"),
        "banked.c must emit BSF FSR,5 for a bank-1 direct access:\n{asm}"
    );
    assert!(
        asm.contains("BCF FSR, 5"),
        "banked.c must emit BCF FSR,5 for bank-0 direct accesses:\n{asm}"
    );
    // The deref of g19 (bank 1) must read through INDF with FSR<5> set.
    let lines: Vec<&str> = asm.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.contains("MOVF INDF, W") {
            let prev = lines[i - 1];
            assert!(
                !prev.contains("BCF FSR, 5"),
                "INDF touch must not be preceded by BCF FSR,5 (would redirect a bank-1 pointer):\n{prev}"
            );
        }
    }
}

/// Regression (epic-cc#325 follow-up): a store of a bank-1 value through a
/// runtime indirect pointer to a bank-0 destination. The value load's
/// `BSF FSR,5` reassert must not clobber the pointer's FSR setup before
/// the INDF store: the value byte is staged in common RAM first. Without
/// the staging, the store lands in bank 1 and buf[0] keeps its preload.
#[test]
fn indirect_store_of_banked_value_lands_in_the_right_bank() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/indirect_store_bank.c");
    p.run(10_000);
    assert!(p.halted());
    assert_eq!(
        p.ram()[globals["buf"] as usize],
        5,
        "buf[0] must be 5: the store through the bank-0 pointer must not be redirected to bank 1"
    );
    assert_eq!(p.ram()[globals["src"] as usize], 5, "src must be 5");
}
