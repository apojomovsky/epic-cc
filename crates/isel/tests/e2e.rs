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

/// epic-cc#1468: startup init of an initialized bank-straddling global
/// must write each byte at its physical address, skipping the common-RAM
/// alias gap between banks, as user-code accesses already assume. A
/// contiguous `base + i` walk from bank 2 hits 0x170, the bank-3 alias
/// of common RAM 0x70, and banking rejects it.
#[test]
fn span_init_c_initialized_straddling_global_survives_startup() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/span_init.c"
    ));
    let zbig = globals["zbig"];
    assert_eq!(
        zbig, 0x160,
        "span_init.c layout: zbig[90] at 0x{zbig:03X} must start near the end of bank 2 and straddle into bank 3"
    );
    p.run(200_000);
    // Bytes past the bank boundary land in bank 3, not the alias gap.
    assert_eq!(p.ram()[0x160], 0x11, "zbig[0] at its physical byte");
    assert_eq!(p.ram()[0x16F], 0x22, "zbig[15] at bank 2's last byte");
    assert_eq!(p.ram()[0x190], 0x33, "zbig[16] at bank 3's first byte");
    assert_eq!(p.ram()[0x1D9], 0x44, "zbig[89] at its physical byte");
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0xAA,
        "out == zbig[0] + zbig[15] + zbig[16] + zbig[89] after startup init"
    );
    assert!(p.halted());
}

#[test]
fn gep_offset_past_255_load_and_store_are_exact() {
    // in == 200: store total 400, load total 300 (epic-cc#1466).
    for fixture in ["gep_offset_wrap.c", "gep_offset_wrap_ptr.c"] {
        let (mut p, globals) = compile(&format!(
            "{}/tests/fixtures/{fixture}",
            env!("CARGO_MANIFEST_DIR")
        ));
        let in_addr = globals["in"] as usize;
        p.ram_mut()[in_addr] = 200;
        p.ram_mut()[in_addr + 1] = 0;
        let big = globals["big"] as usize;
        let phys_of = |offset: usize| {
            let a = (big + offset) & 0x1FF;
            if (0x70..=0x7F).contains(&(a & 0xFF)) {
                a & 0xFF
            } else {
                a
            }
        };
        let wrapped_of = |offset: usize| {
            let a = (big & 0x100) | ((big + offset) & 0xFF);
            if (0x70..=0x7F).contains(&(a & 0xFF)) {
                a & 0xFF
            } else {
                a
            }
        };
        p.ram_mut()[phys_of(300)] = 0x3C;
        p.ram_mut()[wrapped_of(300)] = 0xEE;
        p.run(2_000_000);
        assert!(p.halted(), "{fixture} must halt");
        assert_eq!(
            p.ram()[globals["out"] as usize],
            0x3C,
            "{fixture}: out == byte at big + 300, not its low-byte wrap"
        );
        assert_eq!(
            p.ram()[phys_of(400)],
            0xA5,
            "{fixture}: big + 400 holds the stored 0xA5"
        );
        assert_ne!(
            p.ram()[wrapped_of(400)],
            0xA5,
            "{fixture}: wrapped store address must not hold 0xA5"
        );
    }
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
}

#[test]
fn byval_gep_launder_reads_back_field_byte() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/byval_gep_launder.c"
    );
    let (mut p, globals) = compile(fixture);
    p.run(5000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "the laundered byval field address must read back s.b"
    );
}

#[test]
fn va_arg_gep_index_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/va_arg_gep_index.c"
    ));
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "out == byte written through the va_arg-indexed pointer"
    );
    assert!(p.halted());
}

#[test]
fn cond_term_ptr_arg_c_runs_correctly() {
    // in == 1 (fixture comment) -> out == 0x5A.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cond_term_ptr_arg.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 1;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "out == byte written through the selected pointer"
    );
    assert!(p.halted());
}

#[test]
fn gep_value_icmp_c_runs_correctly() {
    // in == 8 (fixture comment) -> out == 4, out2 == 1.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gep_value_icmp.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 8;
    p.ram_mut()[in_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 4, "out == loop trips");
    assert_eq!(
        p.ram()[globals["out2"] as usize],
        1,
        "out2 == cursor landed on the GEP end"
    );
    assert!(p.halted());
}

#[test]
fn gep_value_icmp_carry_c_runs_correctly() {
    // in == 8 (fixture comment) -> out == 5, out2 == 1.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gep_value_icmp_carry.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 8;
    p.ram_mut()[in_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 5, "out == loop trips");
    assert_eq!(
        p.ram()[globals["out2"] as usize],
        1,
        "out2 == cursor landed on the GEP end"
    );
    assert!(p.halted());
}

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

#[test]
fn term_alias_ptr_arg_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/term_alias_ptr_arg.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 1;
    let in2_addr = globals["in2"] as usize;
    p.ram_mut()[in2_addr] = 0;
    p.ram_mut()[in2_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "out == 0x5A for in == 1, in2 == 0"
    );
    assert!(p.halted());
}

#[test]
fn term_alias_stacked_arg_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/term_alias_stacked_arg.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 1;
    let in2_addr = globals["in2"] as usize;
    p.ram_mut()[in2_addr] = 5;
    p.ram_mut()[in2_addr + 1] = 0;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "out == 0x5A for in == 1, in2 == 5"
    );
    assert_eq!(
        p.ram()[globals["out2"] as usize],
        5,
        "out2 == stacked term value 5"
    );
    assert!(p.halted());
}

#[test]
fn term_alias_va_arg_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/term_alias_va_arg.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 1;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x5A,
        "out == 0x5A for in == 1, va_arg == 5"
    );
    assert!(p.halted());
}
