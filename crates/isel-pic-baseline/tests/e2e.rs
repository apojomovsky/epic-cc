//! P2 end-to-end acceptance: three C programs compiled through the real
//! baseline pipeline (clang -> irparse -> wholeprog -> legalize ->
//! callgraph -> alloc -> isel-pic-baseline -> asm) and executed in the real
//! `PicBaseline` simulator. The baseline routes straight from isel to asm:
//! no schedule, banking, or peephole (the D-2 FSR bank-bit reassertion lives
//! inside isel-pic-baseline, docs/37).
//!
//! `add.c` and `scalar.c` are byte-identical to the driver fixtures, so the
//! hand-computed expected values come from the driver e2e tests of the same
//! C source. `banked.c` is sized for the 509's two 16-byte GPR banks and
//! exercises the D-2 sequencing through real codegen: direct accesses to
//! both banks, an i16 add, indirect accesses to both banks, and an icmp.

use asm::assemble_words;
use device::PIC12F509;
use isel_pic_baseline::select;
use pic14_sim::PicBaseline;
use std::collections::HashMap;
use std::process::Command;
static E2E_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run clang + the full IR pipeline on `c_path`, targeting PIC12F509, and
/// return a freshly constructed (not yet run) `PicBaseline` plus the global
/// address map so each test can seed input addresses by name before calling
/// `.run()`.
fn compile(c_path: &str) -> (PicBaseline, HashMap<String, u16>) {
    let (p, globals, _asm) = compile_asm(c_path);
    (p, globals)
}

/// Like `compile`, but also returns the emitted `.asm` text so a test can
/// inspect the D-2 reassertion the backend emitted.
fn compile_asm(c_path: &str) -> (PicBaseline, HashMap<String, u16>, String) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let (ll, _dep) = clang_compile(&clang, &resdir, c_path);
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC12F509.stack_depth as usize);
    let layout = alloc::allocate(&PIC12F509, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&PIC12F509, &m, &addrs);
    let words = assemble_words(&PIC12F509, &asm);

    (
        PicBaseline::with_device(&PIC12F509, words),
        layout.globals,
        asm,
    )
}

/// Run clang alone on `c_path`, returning the `.ll` text and the
/// dependency-file text (unused here; kept for symmetry with the other
/// backends' e2e helpers).
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
        "clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (String::from_utf8(out.stdout).unwrap(), String::new())
}

#[test]
fn add_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    let (mut sim, globals) = compile("tests/fixtures/add.c");
    sim.ram_mut()[globals["in"] as usize] = 5;
    sim.run(10_000);
    assert!(sim.halted(), "add.c should halt (SLEEP)");
    assert_eq!(sim.ram()[globals["out"] as usize], 6);
}

#[test]
fn scalar_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    let (mut sim, globals) = compile("tests/fixtures/scalar.c");
    sim.ram_mut()[globals["in"] as usize] = 7;
    sim.run(100_000);
    assert!(sim.halted(), "scalar.c should halt (SLEEP)");
    assert_eq!(sim.ram()[globals["out"] as usize], 141);
}

#[test]
fn banked_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    let (mut sim, globals) = compile("tests/fixtures/banked.c");
    sim.run(100_000);
    assert!(sim.halted(), "banked.c should halt (SLEEP)");
    assert_eq!(sim.ram()[globals["buf"] as usize], 9, "buf[0]");
    assert_eq!(sim.ram()[globals["g_bank1"] as usize], 11, "g_bank1");
    assert_eq!(sim.ram()[globals["out"] as usize], 2, "out");
    let s3_lo = sim.ram()[globals["s3"] as usize] as u16;
    let s3_hi = sim.ram()[globals["s3"] as usize + 1] as u16;
    assert_eq!(s3_lo | (s3_hi << 8), 3000, "s3");
}

/// P2's D-2 acceptance (docs/37): the emitted asm must reassert FSR<5>
/// before direct accesses to both banks (a `BSF` for bank 1, a `BCF` for
/// bank 0 after bank 1), and every `INDF` touch must be preceded by an FSR
/// reload. Static inspection, not just simulation: a coincidentally correct
/// byte cannot pass (the sim reads the right bank only because the reassert
/// ran).
#[test]
fn d2_fsr_bank_bit_reassertion_is_emitted() {
    let _guard = E2E_LOCK.lock().unwrap();
    let (_, _, asm) = compile_asm("tests/fixtures/banked.c");
    assert!(
        asm.contains("BSF FSR, 5"),
        "bank-1 direct access must reassert FSR<5>:\n{asm}"
    );
    assert!(
        asm.contains("BCF FSR, 5"),
        "bank-0 direct access must reassert FSR<5>:\n{asm}"
    );
    let fsr_set = asm
        .find("MOVWF FSR")
        .expect("an indirect access must set FSR");
    let indf_store = asm
        .find("MOVWF INDF")
        .expect("an indirect store must write INDF");
    assert!(
        fsr_set < indf_store,
        "FSR must be reloaded before the INDF touch"
    );
}
