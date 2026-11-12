//! End-to-end acceptance for the classic PIC14 backend: C compiled
//! through the real pipeline (clang, irparse, wholeprog, legalize,
//! callgraph, alloc, isel, schedule, banking, peephole, asm) and
//! executed in the `Pic14` simulator.

use asm::assemble_words;
use banking::assign_banks;
use device::PIC16F877A;
use isel::select;
use peephole::optimize;
use pic14_sim::Pic14;
use schedule::schedule;
use std::collections::HashMap;
use std::process::Command;

/// Run clang plus the full IR pipeline on `c_path` for PIC16F877A and
/// return a fresh `Pic14` plus the global address map, so each test can
/// seed input addresses by name before calling `.run()`.
fn compile(c_path: &str) -> (Pic14, HashMap<String, u16>) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = clang_compile(&clang, &resdir, c_path);
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC16F877A.stack_depth as usize);
    let layout = alloc::allocate(&PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&PIC16F877A, &m, &addrs);
    let asm = schedule(&PIC16F877A, &asm);
    let asm = assign_banks(&PIC16F877A, &asm);
    let asm = optimize(&asm);
    isel::verify_page_fit(&m, &asm);
    let words = assemble_words(&PIC16F877A, &asm);
    (Pic14::with_device(&PIC16F877A, words), layout.globals)
}

fn clang_compile(clang: &str, resdir: &str, c_path: &str) -> String {
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
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn multi_term_ptr_arg_c_runs_correctly() {
    // in == 43, in2 == 0xC864, in3 == 260 (fixture comment) -> 0xB6.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multi_term_ptr_arg.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 43;
    p.ram_mut()[in_addr + 1] = 0;
    let in2_addr = globals["in2"] as usize;
    p.ram_mut()[in2_addr] = 0x64;
    p.ram_mut()[in2_addr + 1] = 0xC8;
    let in3_addr = globals["in3"] as usize;
    p.ram_mut()[in3_addr] = 0x04;
    p.ram_mut()[in3_addr + 1] = 0x01;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0xB6,
        "out == hand-computed 0xB6 for in == 43, in2 == 0xC864, in3 == 260"
    );
    // Independent oracle for the two big cells: the physical byte is
    // computed from the alloc map through the simulator's FSR rule
    // (IRP selects the 256-byte half, the common window mirrors), not
    // through any address the compiler emitted, so a dropped carry or
    // term high byte in the shared helper cannot agree with it.
    let big = globals["big"] as usize;
    let phys_of = |offset: usize| {
        let a = (big + offset) & 0x1FF;
        if (0x70..=0x7F).contains(&(a & 0xFF)) {
            a & 0xFF
        } else {
            a
        }
    };
    assert_eq!(
        p.ram()[phys_of(400)],
        0xA5,
        "big + 400 holds put_fwd's 0xA5"
    );
    assert_eq!(
        p.ram()[phys_of(300)],
        0x7C,
        "big + 300 holds put_fwd16's 0x7C"
    );
    assert!(p.halted());
}

/// epic-cc#1433: a pointer laundered through `uintptr_t` lowers as a
/// same-width `trunc i16 -> i16`, and the backend used to refuse it
/// ("trunc must narrow"), aborting the compile.
///
/// The fixture reads the byte back through the laundered address for
/// both a loaded-global pointer and a local's address, so a wrong
/// address reads the wrong byte.
#[test]
fn ptrtoint_laundered_pointer_reads_back_through_its_address() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptrtoint_launder.c"
    );
    let (mut p, globals) = compile(fixture);
    p.run(5000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "the laundered address of a global must read back g's byte"
    );
    assert_eq!(
        p.ram()[globals["out2"] as usize],
        0x33,
        "the laundered address of a local must read back the local's byte"
    );
#[test]
fn gep_k_terms_ptr_value_c_runs_correctly() {
    // in == 23 (fixture comment) -> out == 0x33, out2/out4 == 0x51.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gep_k_terms_ptr_value.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 23;
    p.ram_mut()[in_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x33,
        "out == field read on the small array"
    );
    assert_eq!(
        p.ram()[globals["out2"] as usize],
        0x51,
        "out2 == deref of pointer value stored to a global"
    );
    assert_eq!(
        p.ram()[globals["out4"] as usize],
        0x51,
        "out4 == deref of pointer-param pointer value stored to a global"
    );
    assert!(p.halted());
}

#[test]
fn gep_k_terms_alloca_c_runs_correctly() {
    // in == 23 (fixture comment) -> out == 0x63.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gep_k_terms_alloca.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 23;
    p.ram_mut()[in_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x63,
        "out == deref of alloca pointer value stored to a global"
    );
    assert!(p.halted());
}
