//! P2 end-to-end acceptance: the same four C programs the PIC14 backend used
//! (`add.c`, `scalar.c`, `overlay.c`, `banked.c`), compiled through the real
//! PIC18 pipeline (clang -> irparse -> wholeprog -> legalize -> callgraph ->
//! alloc -> isel-pic18 -> asm) and executed in the real `Pic18` simulator.
//! Mirrors `crates/driver/src/main.rs`'s exact PIC18 pipeline call sequence
//! (driver's own binary doesn't exercise the PIC18 path yet, per Task 14).

use device::PIC18F4550;
use pic14_sim::{parse_hex_pic18, Pic18};
use std::collections::HashMap;
use std::process::Command;

/// Run clang + the full IR pipeline (through `asm::assemble_file_to_hex`) on
/// `c_path`, targeting PIC18F4550, and return a freshly constructed (not yet
/// run) `Pic18` plus the global address map so each test can seed input
/// addresses by name before calling `.run()`.
fn compile(c_path: &str) -> (Pic18, HashMap<String, u16>) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            c_path,
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC18F4550.stack_depth as usize);
    let layout = alloc::allocate(&PIC18F4550, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel_pic18::select(&PIC18F4550, &m, &addrs);
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);

    (Pic18::new(parse_hex_pic18(&hex)), layout.globals)
}

#[test]
fn add_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/add.c"));
    p.ram_mut()[globals["in"] as usize] = 5;
    p.run(200);
    assert_eq!(p.ram()[globals["out"] as usize], 6);
    assert!(p.halted());
}

#[test]
fn scalar_c_runs_correctly() {
    // Hand trace in the fixture's own comment: in = 7 -> n = 7 -> out = 174.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scalar.c"));
    p.ram_mut()[globals["in"] as usize] = 7;
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 174, "out == hand-computed 174 for in == 7");
    assert!(p.halted());
}

#[test]
fn overlay_c_runs_correctly() {
    // No input seeding: `in` stays at its zero-initialized RAM value.
    // big_a(0) = 0+1+2+3+4+5+6+7 = 28.
    // big_b(0+1=1): u0=1-4=-3, u1=-2, u2=-1, u3=0, u4=2, u5=3, u6=4, u7=5 -> sum=8.
    // out = (unsigned char)(28 + 8) = 36.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/overlay.c"));
    p.run(500_000);
    assert_eq!(p.ram()[globals["out"] as usize], 36);
    assert!(p.halted());
}

#[test]
fn banked_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/banked.c"));
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0xFF, "sum 1..90 = 4095, low byte 0xFF");
    assert!(p.halted());
}

/// Step 1's verification: does `banked.c` actually exercise `BSR`-banked
/// addressing on PIC18F4550's Access Bank, or did the fixture's 90-global
/// count stop being enough now that PIC18's Access Bank (92 usable bytes,
/// `0x0004-0x005F`, per `device::PIC18F4550`'s doc comment) is roomier than
/// PIC14's 80-byte bank 0? Empirically: the 91 bytes of globals (90 `g*` +
/// `out`) fit entirely inside the Access Bank (max global address 0x5E),
/// but `main`'s own locals (the loop-free but still substantial spill from
/// summing 90 volatile loads into a 16-bit accumulator) push well past
/// 0x5F, landing as high as 0x111 — so the emitted assembly DOES contain
/// `MOVLB`s, just for locals rather than globals. The fixture's name stays
/// accurate; no global-count bump needed.
#[test]
fn banked_c_asm_contains_movlb() {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args([
            "-target", "msp430", "-O1", "-S", "-emit-llvm", "-ffreestanding", "-nostdinc",
            "-resource-dir", &resdir, "-o", "-",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/banked.c"),
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&PIC18F4550, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel_pic18::select(&PIC18F4550, &m, &addrs);
    assert!(
        asm.lines().any(|l| l.trim().starts_with("MOVLB")),
        "banked.c must exercise BSR-banked addressing on PIC18 (found no MOVLB):\n{asm}"
    );
}

// P3 end-to-end acceptance: the pointer/array/struct fixtures from Tasks
// 3-11, compiled through the real PIC18 pipeline and run in the `Pic18`
// simulator. Seeding and expected values are transcribed verbatim from the
// working PIC14 tests of the same byte-identical C source
// (crates/driver/tests/{array,banked_ptr,structs,ptr_probe}_e2e.rs).

#[test]
fn ptr_probe_pic18_c_runs_correctly() {
    // in = 0x0035; i = 0x35 & 7 = 5; ram[5] = 0x35; out = ram[5] = 0x35.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ptr_probe_pic18.c"));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 0x35; // in low byte
    p.ram_mut()[in_addr + 1] = 0x00; // in high byte
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x35, "out == in's low byte read back through the pointer");
    assert!(p.halted());
}

#[test]
fn array_c_runs_correctly() {
    // Mirrors crates/driver/tests/array_e2e.rs: in low byte = 3 (high byte
    // stays 0) -> buf[3] = 4 -> out = buf[3] = 4.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/array.c"));
    p.ram_mut()[globals["in"] as usize] = 3; // in low byte = 3 (high byte stays 0)
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 4, "out == buf[3] == 3+1");
    assert!(p.halted());
}

#[test]
fn banked_ptr_c_runs_correctly() {
    // Mirrors crates/driver/tests/banked_ptr_e2e.rs: in low byte = 3 (high
    // byte stays 0) -> out == 0xB8 (hand trace in the fixture's comment).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/banked_ptr.c"));
    p.ram_mut()[globals["in"] as usize] = 3; // in low byte = 3 (high byte stays 0)
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0xB8, "out == hand-computed 0xB8 for in == 3");
    assert!(p.halted());
}

#[test]
fn structs_c_runs_correctly() {
    // Mirrors crates/driver/tests/structs_e2e.rs: no input seeding — every
    // value is a fixed constant, so out == 0x4E (hand trace in the
    // fixture's comment).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/structs.c"));
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x4E, "out == hand-computed 0x4E");
    assert!(p.halted());
}
