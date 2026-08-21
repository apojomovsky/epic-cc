//! P2 end-to-end acceptance: the same four C programs the PIC14 backend used
//! (`add.c`, `scalar.c`, `overlay.c`, `banked.c`), compiled through the real
//! PIC18 pipeline (clang -> irparse -> wholeprog -> legalize -> callgraph ->
//! alloc -> isel-pic18 -> asm) and executed in the real `Pic18` simulator.
//! Mirrors `crates/driver/src/main.rs`'s exact PIC18 pipeline call sequence
//! (driver's own binary doesn't exercise the PIC18 path yet, per Task 14).

use device::PIC18F4550;
use pic14_sim::{parse_hex_pic18, Pic18};
use std::collections::HashMap;
static E2E_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
/// 0x5F, landing as high as 0x111, so the emitted assembly DOES contain
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
    // Mirrors crates/driver/tests/structs_e2e.rs: no input seeding, every
    // value is a fixed constant, so out == 0x4E (hand trace in the
    // fixture's comment).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/structs.c"));
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x4E, "out == hand-computed 0x4E");
    assert!(p.halted());
}

// P4 end-to-end acceptance: const (flash) globals read via TBLRD. The two
// fixtures are byte-identical to PIC14's (crates/driver/tests/fixtures/),
// so the expected values come from the PIC14 e2e tests of the same C
// source; on PIC18 the const reads go through TBLRD instead of the RETLW
// tables, and there is no 511-byte chunking.

#[test]
fn const_table_c_runs_correctly() {
    // Mirrors crates/driver/tests/const_table_e2e.rs: in == 290 (0x0122)
    // -> out = (0x33 + 0x02 + 0x3C + 0x11) & 0xFF = 0x82, the four reads
    // exercising chunk-1, chunk-0, chunk-1-last, and chunk-boundary
    // byte offsets of the 300-byte table (PIC18 reads them all linearly
    // via TBLRD; no chunks exist anymore).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/const_table.c"));
    p.ram_mut()[globals["in"] as usize] = 0x22; // 290 = 0x0122, lo byte
    p.ram_mut()[globals["in"] as usize + 1] = 0x01; // hi byte
    p.run(500_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x82, "out == 0x82 for in == 290 (four boundary reads)");
    assert!(p.halted());
}

// P5 end-to-end acceptance: interrupts. The fixtures are byte-identical to
// PIC14's except for the SFR addresses (PORTB 0x06 -> 0xF81, INTCON 0x0B
// -> 0xFF2), so the expected values come from the PIC14 e2e tests of the
// same C source (crates/driver/tests/{interrupt,interrupt_gate}_e2e.rs).

#[test]
fn interrupt_pic18_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    // Mirrors crates/driver/tests/interrupt_e2e.rs: in == 0x10, the ISR
    // fired mid-run after main's PORTB = 0x11 store -> the ISR's
    // bump_isr(out) lands before main's bump reads it:
    //   out = 0x10 -> ISR bumps to 0x11 -> main: bump(0x11)=0x12 -> +1
    //   = 0x13 -> +bump(2)=3 -> 0x16; PORTB ends 0x22.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interrupt_pic18.c"));
    let in_addr = globals["in"] as usize;
    let out_addr = globals["out"] as usize;
    p.ram_mut()[in_addr] = 0x10;

    // Run main to the injection point: right after the `PORTB = 0x11`
    // store (the PIC14 test.s word 77 equivalent, detected by PORTB's
    // value rather than a fixed word count, since the PIC18 layout is
    // instruction-denser).
    let mut steps = 0usize;
    while p.ram()[0xF81] != 0x11 {
        p.step();
        steps += 1;
        assert!(steps < 1000, "never reached the PORTB = 0x11 store (pc = {})", p.pc());
    }
    // The pre-ISR state the hand computation starts from. `out == in`
    // (0x10) is guaranteed: PORTB's store comes after out's store.
    assert_eq!(p.ram()[out_addr], 0x10, "out == in before the ISR");

    // Fire the interrupt: push pc (the next-unexecuted instruction), jump
    // to the high vector 0x0008.
    p.fire_interrupt();
    assert_eq!(p.pc(), 0x0008, "the ISR starts at the high vector");

    // The ISR runs (PORTB = 0x55, out = bump_isr(out)), RETFIE returns to
    // the interrupted instruction, and main completes: out == 0x16, PORTB
    // == 0x22, then the __start SLEEP halts the machine.
    p.run(500_000);
    assert_eq!(
        p.ram()[out_addr],
        0x16,
        "out == hand-computed 0x16 (ISR bump 0x10 -> 0x11, then 0x11 -> 0x12 -> 0x13 -> 0x16)"
    );
    assert_eq!(p.ram()[0xF81], 0x22, "PORTB == 0x22 (main's final SFR write)");
    assert!(p.halted());
}

#[test]
fn ptr_probe_c_runs_correctly() {
    // The ORIGINAL ptr_probe.c (full parity with PIC14, per docs/29's P3
    // note): a runtime RAM pointer AND a const-table read in one program.
    // Mirrors crates/driver/tests/ptr_probe_e2e.rs: in = 1 -> i = 1 ->
    // ram[1] = table[1] = 20 (via TBLRD) -> out = 20.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ptr_probe.c"));
    p.ram_mut()[globals["in"] as usize] = 1; // in = 1 (16-bit; hi byte zero by default)
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 20, "out == table[1] == 20 for in == 1");
    assert!(p.halted());
}

// P6 end-to-end acceptance: i32 (`long`) arithmetic, hardware-multiply
// routine recipes, and the ISR-context routine duplication. Fixtures are
// byte-identical to PIC14's; expected values come from the PIC14 e2e tests
// of the same C source (crates/driver/tests/{long,muldiv,interrupt_mul}_e2e.rs).

#[test]
fn long_c_runs_correctly() {
    // Mirrors crates/driver/tests/long_e2e.rs: in = 0x12345678, sin = -19
    // -> out = 0x1634943A (the whole i32 surface: add/mul/udiv/urem/sdiv/
    // srem/shifts/icmps/casts/struct-byval-sret).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/long.c"));
    p.ram_mut()[globals["in"] as usize] = 0x78; // 0x12345678
    p.ram_mut()[globals["in"] as usize + 1] = 0x56;
    p.ram_mut()[globals["in"] as usize + 2] = 0x34;
    p.ram_mut()[globals["in"] as usize + 3] = 0x12;
    p.ram_mut()[globals["sin"] as usize] = 0xED; // -19 = 0xFFFFFFED
    p.ram_mut()[globals["sin"] as usize + 1] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 2] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 3] = 0xFF;
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x3A, "out == 0x1634943A, byte 0:\n{}", p.ram()[globals["out"] as usize]);
    assert_eq!(p.ram()[globals["out"] as usize + 1], 0x94);
    assert_eq!(p.ram()[globals["out"] as usize + 2], 0x34);
    assert_eq!(p.ram()[globals["out"] as usize + 3], 0x16);
    assert!(p.halted());
}

#[test]
fn muldiv_c_runs_correctly() {
    // Mirrors crates/driver/tests/muldiv_e2e.rs: in = 301 -> out = 210.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/muldiv.c"));
    p.ram_mut()[globals["in"] as usize] = 0x2D; // 301 = 0x012D, lo byte
    p.ram_mut()[globals["in"] as usize + 1] = 0x01; // hi byte
    p.run(500_000);
    assert_eq!(p.ram()[globals["out"] as usize], 210, "out == hand-computed 210");
    assert!(p.halted());
}

#[test]
fn interrupt_mul_pic18_c_runs_correctly() {
    // Mirrors crates/driver/tests/interrupt_mul_e2e.rs: main and the ISR
    // both multiply/divide, so both contexts reach the injected __mul_u8
    // and __udiv_u8 routines; the _isr copies must have disjoint frames.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interrupt_mul_pic18.c"));
    p.ram_mut()[globals["in_a"] as usize] = 47;
    p.ram_mut()[globals["in_b"] as usize] = 5;
    p.ram_mut()[globals["isr_a"] as usize] = 0xAB;
    p.ram_mut()[globals["isr_b"] as usize] = 3;
    p.run(1_000_000);
    // main's context: 47 * 5 = 235 (0xEB), 47 / (5|1) = 47/5 = 9.
    assert_eq!(p.ram()[globals["out"] as usize], 235, "main mul");
    assert_eq!(p.ram()[globals["out_q"] as usize], 9, "main div");
    // The ISR is never fired here (the PIC14 e2e does not fire it either:
    // it asserts the two routine frames are disjoint, which is what makes
    // a mid-routine clobber impossible), so the ISR globals stay untouched.
    assert_eq!(p.ram()[globals["isr_out"] as usize], 0, "ISR frame disjoint from main's");
    assert!(p.halted());
}

#[test]
fn interrupt_gate_pic18_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    // Mirrors crates/driver/tests/interrupt_gate_e2e.rs: the request is
    // latched while INTCON = 0x10 (INT0IE, GIE clear), taken only after
    // main writes INTCON = 0x90. isr_ran == 1, stage == 3, halted.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interrupt_gate_pic18.c"));
    // The request is issued in the stage == 1 window (INTCON not yet
    // written by main). Step until stage == 1 and INTCON == 0x10, then
    // request while masked.
    let mut steps = 0usize;
    while p.ram()[globals["stage"] as usize] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 1000, "never reached stage 1 (pc = {})", p.pc());
    }
    p.request_interrupt();
    assert!(p.interrupt_pending(), "the request must latch while masked");
    p.run(20_000); // through stage 2 (still masked), stage 3's unmask
    assert_eq!(p.ram()[globals["isr_ran"] as usize], 1, "the handler ran exactly once");
    assert_eq!(p.ram()[globals["stage"] as usize], 3, "main completed after the handler returned");
    assert!(p.halted());
}
#[test]
fn long_c_runs_correctly() {
    // Mirrors crates/driver/tests/long_e2e.rs: in = 0x12345678, sin = -19
    // -> out = 0x1634943A (the whole i32 surface: add/mul/udiv/urem/sdiv/
    // srem/shifts/icmps/casts/struct-byval-sret).
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/long.c"));
    p.ram_mut()[globals["in"] as usize] = 0x78; // 0x12345678
    p.ram_mut()[globals["in"] as usize + 1] = 0x56;
    p.ram_mut()[globals["in"] as usize + 2] = 0x34;
    p.ram_mut()[globals["in"] as usize + 3] = 0x12;
    p.ram_mut()[globals["sin"] as usize] = 0xED; // -19 = 0xFFFFFFED
    p.ram_mut()[globals["sin"] as usize + 1] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 2] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 3] = 0xFF;
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x3A, "out == 0x1634943A, byte 0:\n{}", p.ram()[globals["out"] as usize]);
    assert_eq!(p.ram()[globals["out"] as usize + 1], 0x94);
    assert_eq!(p.ram()[globals["out"] as usize + 2], 0x34);
    assert_eq!(p.ram()[globals["out"] as usize + 3], 0x16);
    assert!(p.halted());
}

#[test]
fn muldiv_c_runs_correctly() {
    // Mirrors crates/driver/tests/muldiv_e2e.rs: in = 301 -> out = 210.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/muldiv.c"));
    p.ram_mut()[globals["in"] as usize] = 0x2D; // 301 = 0x012D, lo byte
    p.ram_mut()[globals["in"] as usize + 1] = 0x01; // hi byte
    p.run(500_000);
    assert_eq!(p.ram()[globals["out"] as usize], 210, "out == hand-computed 210");
    assert!(p.halted());
}

#[test]
fn interrupt_mul_pic18_c_runs_correctly() {
    // Mirrors crates/driver/tests/interrupt_mul_e2e.rs: main and the ISR
    // both multiply/divide, so both contexts reach the injected __mul_u8
    // and __udiv_u8 routines; the _isr copies must have disjoint frames.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interrupt_mul_pic18.c"));
    p.ram_mut()[globals["in_a"] as usize] = 47;
    p.ram_mut()[globals["in_b"] as usize] = 5;
    p.ram_mut()[globals["isr_a"] as usize] = 0xAB;
    p.ram_mut()[globals["isr_b"] as usize] = 3;
    p.run(1_000_000);
    // main's context: 47 * 5 = 235 (0xEB), 47 / (5|1) = 47/5 = 9.
    assert_eq!(p.ram()[globals["out"] as usize], 235, "main mul");
    assert_eq!(p.ram()[globals["out_q"] as usize], 9, "main div");
    // The ISR is never fired here (the PIC14 e2e does not fire it either:
    // it asserts the two routine frames are disjoint, which is what makes
    // a mid-routine clobber impossible), so the ISR globals stay untouched.
    assert_eq!(p.ram()[globals["isr_out"] as usize], 0, "ISR frame disjoint from main's");
    assert!(p.halted());
}

#[test]
fn float_c_runs_correctly() {
    // Mirrors crates/driver/tests/float_e2e.rs: in = 3.0f (0x40400000) ->
    // out1 = 3.0/2.5 = 1.2 = 0x3F99999A (RNE), out2 = 9.0 = 0x41100000
    // (via fadd/fmul exact + fptosi/sitofp), out3 = 1.0/3.0 = 0x3EAAAAAB
    // (RNE) via the struct sret/byval path.
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/float.c"));
    // 3.0f = 0x40400000 LE bytes 00 00 40 40
    p.ram_mut()[globals["in"] as usize] = 0x00;
    p.ram_mut()[globals["in"] as usize + 1] = 0x00;
    p.ram_mut()[globals["in"] as usize + 2] = 0x40;
    p.ram_mut()[globals["in"] as usize + 3] = 0x40;
    p.run(2_000_000);
    // out1 = 0x3F99999A LE 9A 99 99 3F
    assert_eq!(p.ram()[globals["out1"] as usize], 0x9A);
    assert_eq!(p.ram()[globals["out1"] as usize + 1], 0x99);
    assert_eq!(p.ram()[globals["out1"] as usize + 2], 0x99);
    assert_eq!(p.ram()[globals["out1"] as usize + 3], 0x3F);
    // out2 = 0x41100000 LE 00 00 10 41
    assert_eq!(p.ram()[globals["out2"] as usize], 0x00);
    assert_eq!(p.ram()[globals["out2"] as usize + 1], 0x00);
    assert_eq!(p.ram()[globals["out2"] as usize + 2], 0x10);
    assert_eq!(p.ram()[globals["out2"] as usize + 3], 0x41);
    // out3 = 0x3EAAAAAB LE AB AA AA 3E
    assert_eq!(p.ram()[globals["out3"] as usize], 0xAB);
    assert_eq!(p.ram()[globals["out3"] as usize + 1], 0xAA);
    assert_eq!(p.ram()[globals["out3"] as usize + 2], 0xAA);
    assert_eq!(p.ram()[globals["out3"] as usize + 3], 0x3E);
    assert!(p.halted());
}
