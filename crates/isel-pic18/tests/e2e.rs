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
/// run) `Pic18` plus the global address map so each test can locate globals
/// by name when asserting on `.run()` results. Input globals carry their
/// values via C initializers in the fixtures (epic-cc#561: `__start` clears
/// zero-initialized RAM before main).
fn compile(c_path: &str) -> (Pic18, HashMap<String, u16>) {
    let (p, globals, _asm) = compile_with_asm(c_path);
    (p, globals)
}

/// The pipeline itself, plus the generated asm text so a test can assert on
/// the instructions selected, not just the simulated result (epic-cc#470:
/// shift byte moves; epic-cc#471: FSR0 seeded once per indirect access).
fn compile_with_asm(c_path: &str) -> (Pic18, HashMap<String, u16>, String) {
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
            "-g",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            c_path,
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll_opts(&ll_text, true);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC18F4550.stack_depth as usize);
    let layout = alloc::allocate(&PIC18F4550, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel_pic18::select_with_locs(
        &PIC18F4550,
        &m,
        &addrs,
        layout.isr_low_save,
        layout.isr_save,
        layout.isr_hi_save,
    )
    .0;
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);

    (Pic18::new(parse_hex_pic18(&hex)), layout.globals, asm)
}

#[test]
fn add_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/add.c"));
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
    // No input seeding: `in` stays at its zero-initialized RAM value.
    // big_a(0) = 0+1+2+3+4+5+6+7 = 28.
    // big_b(0+1=1): u0=1-4=-3, u1=-2, u2=-1, u3=0, u4=2, u5=3, u6=4, u7=5 -> sum=8.
    // out = (unsigned char)(28 + 8) = 36.
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
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/banked.c"
    ));
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0xFF,
        "sum 1..90 = 4095, low byte 0xFF"
    );
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
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-g",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/banked.c"),
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let ll_text = String::from_utf8(ll.stdout).unwrap();
    let mut m = irparse::parse_ll_opts(&ll_text, true);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&PIC18F4550, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel_pic18::select_with_locs(
        &PIC18F4550,
        &m,
        &addrs,
        layout.isr_low_save,
        layout.isr_save,
        layout.isr_hi_save,
    )
    .0;
    assert!(
        asm.lines().any(|l| l.trim().starts_with("MOVLB")),
        "banked.c must exercise BSR-banked addressing on PIC18 (found no MOVLB):\n{asm}"
    );
}

// P3 end-to-end acceptance: the pointer/array/struct fixtures from Tasks
// 3-11, compiled through the real PIC18 pipeline and run in the `Pic18`
// simulator. Input and expected values are transcribed verbatim from the
// working PIC14 tests of the same byte-identical C source
// (crates/driver/tests/{array,banked_ptr,structs,ptr_probe}_e2e.rs); the
// inputs ride in the fixtures' initializers (epic-cc#561 clears
// zero-initialized RAM before main).

#[test]
fn ptr_probe_pic18_c_runs_correctly() {
    // in = 0x0035; i = 0x35 & 7 = 5; ram[5] = 0x35; out = ram[5] = 0x35.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_probe_pic18.c"
    ));
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
    // Mirrors crates/driver/tests/array_e2e.rs: in low byte = 3 (high byte
    // stays 0) -> buf[3] = 4 -> out = buf[3] = 4.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/array.c"
    ));
    p.run(200_000);
    assert_eq!(p.ram()[globals["out"] as usize], 4, "out == buf[3] == 3+1");
    assert!(p.halted());
}

#[test]
fn banked_ptr_c_runs_correctly() {
    // Mirrors crates/driver/tests/banked_ptr_e2e.rs: in low byte = 3 (high
    // byte stays 0) -> out == 0xB8 (hand trace in the fixture's comment).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/banked_ptr.c"
    ));
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0xB8,
        "out == hand-computed 0xB8 for in == 3"
    );
    assert!(p.halted());
}

#[test]
fn struct_global_index_c_runs_correctly() {
    // epic-cc#468: struct copy into a runtime-indexed global array
    // element plus the address of such an element taken as a value;
    // isel-pic18 panicked materializing that GEP-over-Global address.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/struct_global_index.c"
    ));
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x9A,
        "out == hand-computed 0x9A"
    );
    assert!(p.halted());
    // The stored element address must route through the scaled-index
    // arm: seed FSR0, read back FSR0L (8-bit SFR-segment form 0x0E9).
    // (The copy itself walks POSTINC0/POSTINC1 off FSR-seeded pointers
    // and materializes no value.) A future routing change that folds
    // the address move away reads 0 instead of 1.
    let fsr0l_readbacks = asm.matches("MOVF 0x0E9,W,A").count();
    assert_eq!(
        fsr0l_readbacks, 1,
        "expected 1 FSR0L read-back (stored element address):\n{asm}"
    );
}

#[test]
fn structs_c_runs_correctly() {
    // Mirrors crates/driver/tests/structs_e2e.rs: no input seeding, every
    // value is a fixed constant, so out == 0x4E (hand trace in the
    // fixture's comment).
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
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/const_table.c"
    ));
    p.run(500_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x82,
        "out == 0x82 for in == 290 (four boundary reads)"
    );
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
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt_pic18.c"
    ));
    let out_addr = globals["out"] as usize;

    // Run main to the injection point: right after the `PORTB = 0x11`
    // store (the PIC14 test.s word 77 equivalent, detected by PORTB's
    // value rather than a fixed word count, since the PIC18 layout is
    // instruction-denser).
    let mut steps = 0usize;
    while p.ram()[0xF81] != 0x11 {
        p.step();
        steps += 1;
        assert!(
            steps < 1000,
            "never reached the PORTB = 0x11 store (pc = {})",
            p.pc()
        );
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
    assert_eq!(
        p.ram()[0xF81],
        0x22,
        "PORTB == 0x22 (main's final SFR write)"
    );
    assert!(p.halted());
}

#[test]
fn compat_isr_preserves_fsr0h_across_w_save() {
    // epic-cc#356 regression: the compat (non-priority) ISR prologue wrote
    // W to the literal address 0x0004, which on PIC18F4550
    // (`fixed_retval` starting at 0x0000) is `common_lo + 4`, the very
    // slot the same prologue had just used to snapshot FSR0H. The W store
    // clobbered that snapshot, so the epilogue restored FSR0H from
    // whatever W held at interrupt entry instead of the preempted
    // main-context pointer's high byte. Hand-built IR (no clang): a lone
    // ISR gets the fixed-block compat prologue regardless of body.
    let m = ir::parse(
        "fn isr(void) [isr] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    // A lone ISR needs its PROD/FSR1 save area (epic-cc#477); any
    // disjoint RAM address works for this hand-built module.
    let asm =
        isel_pic18::select_with_locs(&PIC18F4550, &m, &HashMap::new(), None, Some(0x0040), None).0;
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);
    let mut p = Pic18::new(parse_hex_pic18(&hex));

    // Simulate main mid-pointer-dereference: FSR0H loaded with a pointer's
    // high byte above 0xFF. (FSR0L shares the same save/restore shape but
    // aliases the retval-backup restore order bug tracked separately as
    // epic-cc#362; asserting it here would conflate the two.)
    p.ram_mut()[0xFEA] = 0xAB; // FSR0H

    p.fire_interrupt();
    assert_eq!(p.pc(), 8, "ISR starts at the high vector");

    // Step exactly through the prologue/body/epilogue: RETFIE pops the
    // pushed return address (0, the injection point), so pc hitting 0
    // again means the ISR just returned.
    let mut steps = 0;
    loop {
        p.step();
        steps += 1;
        assert!(steps < 200, "ISR never returned (pc = {})", p.pc());
        if p.pc() == 0 {
            break;
        }
    }

    assert_eq!(p.ram()[0xFEA], 0xAB, "FSR0H must survive the compat ISR");
}

#[test]
fn compat_isr_preserves_status_bsr_fsr0l_across_retval_backup() {
    // epic-cc#362 regression: the compat epilogue restored the retval
    // backup (common_lo+12..+15 -> common_lo..+3) before restoring
    // STATUS/BSR/FSR0L (common_lo+1..+3 -> the real SFRs). The two
    // destination ranges alias, so the SFR restore read back whatever the
    // retval backup had just written. Hand-built IR (no clang): a lone
    // ISR gets the fixed-block compat prologue regardless of body.
    let m = ir::parse(
        "fn isr(void) [isr] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    // A lone ISR needs its PROD/FSR1 save area (epic-cc#477); any
    // disjoint RAM address works for this hand-built module.
    let asm =
        isel_pic18::select_with_locs(&PIC18F4550, &m, &HashMap::new(), None, Some(0x0040), None).0;
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);
    let mut p = Pic18::new(parse_hex_pic18(&hex));

    p.ram_mut()[0xFD8] = 0x93; // STATUS
    p.ram_mut()[0xFE0] = 0x27; // BSR
    p.ram_mut()[0xFE9] = 0x5C; // FSR0L

    p.fire_interrupt();
    assert_eq!(p.pc(), 8, "ISR starts at the high vector");

    let mut steps = 0;
    loop {
        p.step();
        steps += 1;
        assert!(steps < 200, "ISR never returned (pc = {})", p.pc());
        if p.pc() == 0 {
            break;
        }
    }

    // Z/N included: the epilogue restores W before STATUS, so the final
    // MOVF cannot clobber the restored flags (epic-cc#604).
    assert_eq!(
        p.ram()[0xFD8],
        0x93,
        "STATUS (including Z/N) must survive the compat ISR"
    );
    assert_eq!(p.ram()[0xFE0], 0x27, "BSR must survive the compat ISR");
    assert_eq!(p.ram()[0xFE9], 0x5C, "FSR0L must survive the compat ISR");
}

#[test]
fn compat_isr_preserves_zero_flag_when_w_is_nonzero() {
    // epic-cc#604 regression: the compat epilogue restored STATUS before W,
    // and the W restore via MOVF sets Z/N from W. A Timer2 IRQ landing in a
    // main-line XORLW/BNZ indirect-call dispatch window then mis-dispatched.
    // W nonzero with Z set separates the two orders: W-last returns Z clear.
    let m = ir::parse(
        "fn isr(void) [isr] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let asm =
        isel_pic18::select_with_locs(&PIC18F4550, &m, &HashMap::new(), None, Some(0x0040), None).0;
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);
    let mut p = Pic18::new(parse_hex_pic18(&hex));

    p.set_w(0x55);
    p.ram_mut()[0xFD8] = 0x1D; // STATUS with C, Z, OV, N set

    p.fire_interrupt();
    assert_eq!(p.pc(), 8, "ISR starts at the high vector");

    let mut steps = 0;
    loop {
        p.step();
        steps += 1;
        assert!(steps < 200, "ISR never returned (pc = {})", p.pc());
        if p.pc() == 0 {
            break;
        }
    }

    assert_eq!(p.w(), 0x55, "W must survive the compat ISR");
    assert_eq!(
        p.ram()[0xFD8],
        0x1D,
        "STATUS (including Z/N) must survive the compat ISR"
    );
}

#[test]
fn ptr_probe_c_runs_correctly() {
    // The ORIGINAL ptr_probe.c (full parity with PIC14, per docs/29's P3
    // note): a runtime RAM pointer AND a const-table read in one program.
    // Mirrors crates/driver/tests/ptr_probe_e2e.rs: in = 1 -> i = 1 ->
    // ram[1] = table[1] = 20 (via TBLRD) -> out = 20.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_probe.c"
    ));
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        20,
        "out == table[1] == 20 for in == 1"
    );
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
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/long.c"
    ));
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x3A,
        "out == 0x1634943A, byte 0:\n{}",
        p.ram()[globals["out"] as usize]
    );
    assert_eq!(p.ram()[globals["out"] as usize + 1], 0x94);
    assert_eq!(p.ram()[globals["out"] as usize + 2], 0x34);
    assert_eq!(p.ram()[globals["out"] as usize + 3], 0x16);
    assert!(p.halted());
}

#[test]
fn muldiv_c_runs_correctly() {
    // Mirrors crates/driver/tests/muldiv_e2e.rs: in = 301 -> out = 210.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/muldiv.c"
    ));
    p.run(500_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        210,
        "out == hand-computed 210"
    );
    assert!(p.halted());
}

#[test]
fn interrupt_mul_pic18_c_runs_correctly() {
    // Mirrors crates/driver/tests/interrupt_mul_e2e.rs: main and the ISR
    // both multiply/divide, so both contexts reach the injected __mul_u8
    // and __udiv_u8 routines; the _isr copies must have disjoint frames.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt_mul_pic18.c"
    ));
    p.run(1_000_000);
    // main's context: 47 * 5 = 235 (0xEB), 47 / (5|1) = 47/5 = 9.
    assert_eq!(p.ram()[globals["out"] as usize], 235, "main mul");
    assert_eq!(p.ram()[globals["out_q"] as usize], 9, "main div");
    // The ISR is never fired here (the PIC14 e2e does not fire it either:
    // it asserts the two routine frames are disjoint, which is what makes
    // a mid-routine clobber impossible), so the ISR globals stay untouched.
    assert_eq!(
        p.ram()[globals["isr_out"] as usize],
        0,
        "ISR frame disjoint from main's"
    );
    assert!(p.halted());
}

#[test]
fn interrupt_gate_pic18_c_runs_correctly() {
    let _guard = E2E_LOCK.lock().unwrap();
    // Mirrors crates/driver/tests/interrupt_gate_e2e.rs: the request is
    // latched while INTCON = 0x10 (INT0IE, GIE clear), taken only after
    // main writes INTCON = 0x90. isr_ran == 1, stage == 3, halted.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt_gate_pic18.c"
    ));
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
    assert_eq!(
        p.ram()[globals["isr_ran"] as usize],
        1,
        "the handler ran exactly once"
    );
    assert_eq!(
        p.ram()[globals["stage"] as usize],
        3,
        "main completed after the handler returned"
    );
    assert!(p.halted());
}

#[test]
fn float_c_runs_correctly() {
    // Mirrors crates/driver/tests/float_e2e.rs: in = 3.0f (0x40400000) ->
    // out1 = 3.0/2.5 = 1.2 = 0x3F99999A (RNE), out2 = 9.0 = 0x41100000
    // (via fadd/fmul exact + fptosi/sitofp), out3 = 1.0/3.0 = 0x3EAAAAAB
    // (RNE) via the struct sret/byval path.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/float.c"
    ));
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

#[test]
fn shift_bytes_c_runs_correctly_and_skips_the_rotate_loop() {
    // epic-cc#470: a multiple-of-8 shift must lower to byte moves, not a
    // bit-serial rotate loop. u16in=0xBEEF, u32in=0x12345678, s16in=0xEDCC.
    // Expected (hand-computed, see shift_bytes.c): u16_lshr8=0x00BE,
    // u16_shl8=0xEF00, u32_lshr16=0x1234, u32_shl16=0x56780000,
    // u32_lshr11=0x2468A, s16_ashr8=0xFFED (sign-extended -19),
    // u32_shl12=0x45678000, s16_ashr12=0xFFFE (sign-extended -2). The
    // three non-multiple-of-8 cases exercise the residual rotate for
    // every op, including the r > 0 index bases the multiples never hit.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/shift_bytes.c"
    ));
    p.run(500_000);

    assert_eq!(p.ram()[globals["u16_lshr8"] as usize], 0xBE);
    assert_eq!(p.ram()[globals["u16_lshr8"] as usize + 1], 0x00);

    assert_eq!(p.ram()[globals["u16_shl8"] as usize], 0x00);
    assert_eq!(p.ram()[globals["u16_shl8"] as usize + 1], 0xEF);

    assert_eq!(p.ram()[globals["u32_lshr16"] as usize], 0x34);
    assert_eq!(p.ram()[globals["u32_lshr16"] as usize + 1], 0x12);
    assert_eq!(p.ram()[globals["u32_lshr16"] as usize + 2], 0x00);
    assert_eq!(p.ram()[globals["u32_lshr16"] as usize + 3], 0x00);

    assert_eq!(p.ram()[globals["u32_shl16"] as usize], 0x00);
    assert_eq!(p.ram()[globals["u32_shl16"] as usize + 1], 0x00);
    assert_eq!(p.ram()[globals["u32_shl16"] as usize + 2], 0x78);
    assert_eq!(p.ram()[globals["u32_shl16"] as usize + 3], 0x56);

    assert_eq!(p.ram()[globals["u32_lshr11"] as usize], 0x8A);
    assert_eq!(p.ram()[globals["u32_lshr11"] as usize + 1], 0x46);
    assert_eq!(p.ram()[globals["u32_lshr11"] as usize + 2], 0x02);
    assert_eq!(p.ram()[globals["u32_lshr11"] as usize + 3], 0x00);

    assert_eq!(p.ram()[globals["s16_ashr8"] as usize], 0xED);
    assert_eq!(p.ram()[globals["s16_ashr8"] as usize + 1], 0xFF);

    assert_eq!(p.ram()[globals["u32_shl12"] as usize], 0x00);
    assert_eq!(p.ram()[globals["u32_shl12"] as usize + 1], 0x80);
    assert_eq!(p.ram()[globals["u32_shl12"] as usize + 2], 0x67);
    assert_eq!(p.ram()[globals["u32_shl12"] as usize + 3], 0x45);

    assert_eq!(p.ram()[globals["s16_ashr12"] as usize], 0xFE);
    assert_eq!(p.ram()[globals["s16_ashr12"] as usize + 1], 0xFF);

    assert!(p.halted());

    // RLCF exactly 12: only u32_shl12 (<<12 = 8*1 + 4) has a nonzero
    // shl residual, 4 rotate iterations over the 3 bytes the byte-move
    // left live. The exact-multiple shl cases (u16_shl8, u32_shl16)
    // must contribute zero: a higher count means a multiple-of-8 shift
    // is still falling through to the bit-serial loop.
    let rlcf_count = asm.matches("RLCF").count();
    assert_eq!(
        rlcf_count, 12,
        "expected exactly 12 RLCF (u32_shl12's 4 iterations x 3 active \
         bytes only), got {rlcf_count}:\n{asm}"
    );
    // RRCF exactly 13: u32_lshr11 (>>11 = 8*1 + 3, 3 iterations x 3
    // active bytes) and s16_ashr12 (>>12 = 8*1 + 4, 4 iterations over
    // the 1 surviving byte). u16_lshr8 and s16_ashr8 (both r == 0) must
    // contribute zero: a higher count means a multiple-of-8 case is
    // still falling through to the bit-serial loop.
    let rrcf_count = asm.matches("RRCF").count();
    assert_eq!(
        rrcf_count, 13,
        "expected exactly 13 RRCF (u32_lshr11's 3x3 plus s16_ashr12's \
         4x1), got {rrcf_count}:\n{asm}"
    );
}

#[test]
fn ptr_postinc_c_runs_correctly_and_seeds_fsr0_once() {
    // epic-cc#471: `*p = 0x12345678UL` and `out32 = *q` through runtime
    // pointers must each seed FSR0 (FSR0L/FSR0H, 0xFE9/0xFEA) exactly once
    // and walk the remaining bytes with POSTINC0 (0xFEE), not re-seed FSR0
    // from scratch per byte.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_postinc.c"
    ));

    let buf32 = globals["buf32"] as usize;

    p.run(2_000);

    // Store: buf32 must hold the stored 0x12345678, little-endian.
    assert_eq!(p.ram()[buf32], 0x78);
    assert_eq!(p.ram()[buf32 + 1], 0x56);
    assert_eq!(p.ram()[buf32 + 2], 0x34);
    assert_eq!(p.ram()[buf32 + 3], 0x12);

    // Load: out32 must hold what buf32in held.
    let out32 = globals["out32"] as usize;
    assert_eq!(p.ram()[out32], 0x78);
    assert_eq!(p.ram()[out32 + 1], 0x56);
    assert_eq!(p.ram()[out32 + 2], 0x34);
    assert_eq!(p.ram()[out32 + 3], 0x12);

    assert!(p.halted());

    // FSR0L/FSR0H must be seeded exactly once per access (once for the
    // store, once for the load) -- before the fix each 4-byte access
    // re-seeded FSR0 four times, so this would read 4 instead of 1 each.
    let fsr0l_seeds = asm.matches("0xFE9").count();
    let fsr0h_seeds = asm.matches("0xFEA").count();
    assert_eq!(
        fsr0l_seeds, 2,
        "expected FSR0L seeded exactly once per access (store + load):\n{asm}"
    );
    assert_eq!(
        fsr0h_seeds, 2,
        "expected FSR0H seeded exactly once per access (store + load):\n{asm}"
    );

    // The remaining 3 bytes of each 4-byte access (store and load) must
    // walk POSTINC0, not re-seed FSR0: 3 POSTINC0 uses per access, 6 total.
    // Counted in main's body only: __start's zero-clear loop also walks
    // POSTINC0 (CLRF 0xFEE,A) while wiping the zero-init globals.
    let main_asm = asm.split("__start:").next().unwrap();
    let postinc_uses = main_asm.matches("0xFEE").count();
    assert_eq!(
        postinc_uses, 6,
        "expected 6 POSTINC0 uses (3 per 4-byte access x 2 accesses):\n{asm}"
    );
}

#[test]
fn stride_chain_c_runs_correctly() {
    // Runtime indices over a 12-byte-stride struct array: the volatile
    // RAM accesses scale through the FSR0 shift-add chain, and the flash
    // const reads scale through the TBLPTR chain (stride 12 clears the
    // 3-byte gate: 39 + 2 <= 72 naive words). Every access must land
    // exactly where the unrolled adds did.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stride_chain.c"
    ));
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x4E,
        "out == hand-computed 0x4E (fixture comment's trace)"
    );
    // The written element itself: recs[3] = base + 3*12, val = 0x1234.
    let recs = globals["recs"] as usize;
    assert_eq!(p.ram()[recs + 3 * 12], 0x34, "recs[3].val low byte");
    assert_eq!(p.ram()[recs + 3 * 12 + 1], 0x12, "recs[3].val high byte");
    assert_eq!(p.ram()[recs + 3 * 12 + 2], 0x00, "recs[3].tag untouched");
    assert_eq!(
        p.ram()[recs + 5 * 12 + 2],
        7,
        "recs[5].tag written via the constant index"
    );
    assert!(p.halted());
}

#[test]
fn ptr_param_stride_chain_c_folds_the_pointer_into_the_chain() {
    // epic-cc#469 review fix: a runtime POINTER PARAMETER (SlotValue),
    // not a fixed global, indexed with a wide-enough stride to trigger
    // the chain. It must still add the pointer's own two bytes onto the
    // chain-scaled pair -- dropping that landed writes near address 0.
    // Two call sites, different pointers: keeps the parameter from
    // constant-folding to a single global (the unrelated, already-fine
    // Absolute-origin path).
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_param_stride_chain.c"
    ));
    // A 12-byte stride with a 16-bit index takes the MULWF form
    // (epic-cc#477): `MOVLW 12; MULWF idx` then the PRODL/PRODH bytes
    // folded onto the FSR0 pair. The scale-12 chain (17+ words) is no
    // longer the winner at this stride; the sim assertions below are what
    // pin correctness.
    assert!(
        asm.contains("MULWF") && asm.contains("MOVLW 0x0C"),
        "expected the MULWF scaling for a 12-byte stride:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0xFF3,W,A") && asm.contains("MOVF 0xFF4,W,A"),
        "the product must fold PRODL and PRODH onto the FSR0 pair:\n{asm}"
    );
    p.run(2_000_000);
    assert!(p.halted());
    let storage_a = globals["storage_a"] as usize;
    let storage_b = globals["storage_b"] as usize;
    assert_eq!(
        p.ram()[storage_a + 2 * 12 + 2],
        0,
        "storage_a[2].tag must stay untouched (touch was called on storage_b)"
    );
    assert_eq!(
        p.ram()[storage_b + 2 * 12 + 2],
        0x42,
        "storage_b[2].tag must be written at the correct element, not near address 0"
    );
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x42,
        "out == storage_a[2].tag (0) + storage_b[2].tag (0x42)"
    );
}

#[test]
fn ptr_fields_reuse_c_runs_correctly_and_reuses_fsr0() {
    // epic-cc#472: `p->a = 1; p->b = 2; p->c = 3; p->d = 4;` through the
    // same unchanged runtime pointer must seed FSR0's base (FSR0L/FSR0H,
    // 0xFE9/0xFEA) exactly once, for the first field, and reuse it for the
    // other three via a forward delta add (ADDWF/ADDWFC against FSR0L/H's
    // 8-bit SFR-segment form, 0x0E9/0x0EA) instead of re-deriving the
    // address from scratch each time.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_fields_reuse.c"
    ));

    let buf = globals["buf"] as usize;

    p.run(2_000);

    assert_eq!(p.ram()[buf], 1, "buf.a");
    assert_eq!(p.ram()[buf + 1], 2, "buf.b");
    assert_eq!(p.ram()[buf + 2], 3, "buf.c");
    assert_eq!(p.ram()[buf + 3], 4, "buf.d");
    assert!(p.halted());

    // FSR0L/FSR0H must be seeded from the pointer's slot exactly once
    // (for field `a`) -- before the fix each of the 4 field stores
    // re-seeded FSR0 independently, so this would read 4 instead of 1.
    let fsr0l_seeds = asm.matches("0xFE9").count();
    let fsr0h_seeds = asm.matches("0xFEA").count();
    assert_eq!(
        fsr0l_seeds, 1,
        "expected FSR0L seeded exactly once (first field only):\n{asm}"
    );
    assert_eq!(
        fsr0h_seeds, 1,
        "expected FSR0H seeded exactly once (first field only):\n{asm}"
    );

    // The other 3 fields (b, c, d) must each reuse FSR0 via a forward
    // delta add instead of a full reload: 3 ADDWF/ADDWFC pairs against
    // FSR0L/FSR0H's 8-bit SFR-segment form.
    let delta_lo = asm.matches("0x0E9").count();
    let delta_hi = asm.matches("0x0EA").count();
    assert_eq!(
        delta_lo, 3,
        "expected 3 forward-delta adds to FSR0L (fields b, c, d):\n{asm}"
    );
    assert_eq!(
        delta_hi, 3,
        "expected 3 forward-delta adds to FSR0H (fields b, c, d):\n{asm}"
    );

    // Every field write still goes through INDF0 (0xFEF): reuse only
    // changes how FSR0 gets to the right address, never the access itself.
    let indf0_writes = asm.matches("0xFEF").count();
    assert_eq!(
        indf0_writes, 4,
        "expected 4 INDF0 writes (one per field):\n{asm}"
    );
}

#[test]
fn ptr_call_forward_c_runs_correctly_and_skips_fsr0() {
    // epic-cc#473: `fwd(s_t *p) { callee(p); }` forwarding its own pointer
    // param as a call argument must copy the two bytes directly into
    // `callee`'s param slot, not round-trip them through FSR0L/FSR0H
    // (0xFE9/0xFEA). Same for `main`'s `fwd(vp)` (a loaded global pointer
    // forwarded straight into a call). The only place FSR0 is genuinely
    // needed is inside `callee`, which actually dereferences the pointer.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_call_forward.c"
    ));

    let buf = globals["buf"] as usize;

    p.run(2_000);

    assert_eq!(p.ram()[buf], 1, "buf.a");
    assert!(p.halted());

    // FSR0L/FSR0H must be seeded exactly once in the whole program: inside
    // `callee`, to dereference `p->a`. Before the fix, `fwd`'s forwarding
    // of its own param and `main`'s forwarding of the loaded global each
    // added their own (unneeded) FSR0 round-trip, so this would read 3
    // instead of 1.
    let fsr0l_seeds = asm.matches("0xFE9").count();
    let fsr0h_seeds = asm.matches("0xFEA").count();
    assert_eq!(
        fsr0l_seeds, 1,
        "expected FSR0L touched exactly once (inside callee only):\n{asm}"
    );
    assert_eq!(
        fsr0h_seeds, 1,
        "expected FSR0H touched exactly once (inside callee only):\n{asm}"
    );
}

#[test]
fn float_frames_above_the_access_window_select_the_bank() {
    // The narrow-to-wide float conversion fills (__uitofp_f32's CLRF
    // loop, __sitofp_f32's MOVF/MOVWF fill and its BTFSC+MOVLW 0xFF sign
    // fill) address the callee's `val` param slot, which sits wherever
    // the overlay put the routine's frame: the 0x60-byte pad forces every
    // frame past the access window, so the fill must go through the bank
    // select. The sim proves the conversions: in = 3.0f gives 4.0 + 5.0
    // + 6.0 - 1.0 = 14.0f (0x41600000).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/float_frame_high.c"
    ));
    p.run(2_000_000);
    // out = 14.0f = 0x41600000 LE 00 00 60 41
    assert_eq!(p.ram()[globals["out"] as usize], 0x00);
    assert_eq!(p.ram()[globals["out"] as usize + 1], 0x00);
    assert_eq!(p.ram()[globals["out"] as usize + 2], 0x60);
    assert_eq!(p.ram()[globals["out"] as usize + 3], 0x41);
    assert_eq!(p.ram()[globals["sc"] as usize], 0xFF, "sc holds -1");
    assert!(p.halted());
}

#[test]
fn switch_dense_table_runs_correctly() {
    // epic-cc#479: two dense switches lower to PCL jump tables, one
    // base-0 and one base-4 with padding; the run drives every case
    // plus each default through the real simulator, so a wrong table
    // entry, bound, offset, or trampoline copy fails the byte
    // assertions below. The whole-program pipeline folds the inlined
    // case bodies into a value phi, so every table edge runs through
    // a trampoline: this gates the copies too.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/switch_dense.c"
    ));
    assert!(
        asm.contains("ADDWF 0xFF9,F,A"),
        "expected a PCL computed-jump dispatch:\n{asm}"
    );
    assert_eq!(
        asm.matches(".pcltbl").count(),
        2,
        "one page-checked table per surviving dispatch:\n{asm}"
    );
    p.run(500_000);
    // dispatch: h_k(k) for k in 0..7, 99 for the default (k == 8).
    let expect: [u8; 9] = [10, 3, 88, 103, 196, 12, 134, 207, 99];
    for (k, want) in expect.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["results"] as usize + k],
            *want,
            "dispatch({k}) into results[{k}]"
        );
    }
    // dispatch2 (base 4): k == 3 takes the default (7); cases 4..10
    // dispatch their values. This gates the low-bound reject and the
    // padding entries.
    let expect2: [u8; 8] = [7, 44, 10, 53, 193, 17, 137, 210];
    for (i, want) in expect2.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["results2"] as usize + i],
            *want,
            "dispatch2({}) into results2[{i}]",
            i + 3
        );
    }
    // Each case also writes a second global; asserting it pins per-case
    // side effects, not just the dispatched value. epic-cc#497 reported
    // those vanishing on switch-shaped code and no other test covers them.
    let markers: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    for (k, want) in markers.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["markers"] as usize + k],
            *want,
            "dispatch({k})'s second store into markers[{k}]"
        );
    }
    let markers2: [u8; 8] = [19, 11, 12, 13, 14, 15, 16, 17];
    for (i, want) in markers2.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["markers2"] as usize + i],
            *want,
            "dispatch2({})'s second store into markers2[{i}]",
            i + 3
        );
    }
    // No i16 dispatch here: clang folds an i16 switch of this shape
    // into a const lookup before isel runs, so the high-byte reject
    // is pinned by unit test (`i16_dense_switch_checks_the_high_byte`)
    // instead of through the simulator.
    assert!(p.halted());
}

#[test]
fn switch_sparse_tail_tables_the_run_and_chains_the_tail() {
    // epic-cc#578: a dense 0..5 run under a sparse 200 tail lowers to
    // one PCL table for the run plus a compare chain behind the table
    // default. The run drives every table entry, the holes around the
    // tail, the sparse hit, and the far default through the simulator,
    // so a wrong entry, bound, offset, or residual compare fails a byte
    // assertion below.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/switch_sparse_tail.c"
    ));
    assert!(
        asm.contains("ADDWF 0xFF9,F,A"),
        "expected a PCL computed-jump dispatch:\n{asm}"
    );
    assert_eq!(
        asm.matches(".pcltbl").count(),
        1,
        "one page-checked table for the dense run:\n{asm}"
    );
    p.run(2_000_000);
    // h0..h5 on 0..5, default 99 on the holes and far values, hs on 200.
    let expect: [u8; 12] = [10, 3, 88, 103, 196, 12, 99, 99, 99, 146, 99, 99];
    let keys: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 100, 199, 200, 201, 255];
    for (i, want) in expect.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["results"] as usize + i],
            *want,
            "dispatch({}) into results[{i}]",
            keys[i]
        );
    }
    let markers: [u8; 12] = [1, 2, 3, 4, 5, 6, 8, 8, 8, 7, 8, 8];
    for (i, want) in markers.iter().enumerate() {
        assert_eq!(
            p.ram()[globals["markers"] as usize + i],
            *want,
            "dispatch({})'s second store into markers[{i}]",
            keys[i]
        );
    }
    assert!(p.halted());
}

#[test]
fn routine_frame_straddling_a_bsr_bank_is_snapped_and_runs() {
    // epic-cc#509: PIC18's `operand()` banks on the 256-byte BSR boundary,
    // but every PIC18 device declares its whole RAM as one `ram_banks`
    // region, so `round_if_routine`'s region-based check never fired and a
    // frame crossing 0x100 got a MOVLB emitted inside its own skip-sensitive
    // recipe. The fixture's overlay puts `__add_f32` astride 0x100 (base
    // 0xEE..0x103 before the fix); alloc must snap it to 0x100 instead.
    let (mut p, globals, asm) = compile_with_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/routine_frame_straddle.c"
    ));
    // No MOVLB immediately followed by nothing, and no MOVLB between a skip
    // and the instruction it skips: the two are the same failure here.
    let lines: Vec<&str> = asm.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let op = l.split_whitespace().next().unwrap_or("");
        if matches!(
            op,
            "BTFSC" | "BTFSS" | "INCFSZ" | "INFSNZ" | "DECFSZ" | "DCFSNZ"
        ) {
            assert!(
                !lines
                    .get(i + 1)
                    .map(|n| n.trim_start().starts_with("MOVLB"))
                    .unwrap_or(false),
                "a MOVLB lands inside a skip window at line {}:\n{asm}",
                i + 1
            );
        }
    }
    p.run(2_000_000);
    let o = globals["out"] as usize;
    // 3.0 + 1.5 + (float)pad[7], pad[7] = 7 + 3 = 10 -> 14.5f.
    assert_eq!(
        [p.ram()[o], p.ram()[o + 1], p.ram()[o + 2], p.ram()[o + 3]],
        [0x00, 0x00, 0x68, 0x41],
        "out must be 14.5f"
    );
    assert!(p.halted());
}

/// epic-cc#502 acceptance: a volatile global read twice, with a store in
/// between, must reach the file register both times. The cache tracks slot
/// addresses only, and a global's address is deliberately never tracked:
/// an ISR can rewrite it while the epilogue restores the interrupted W, so
/// a cached byte would be stale. The frame-slot elision is covered by the
/// unit tests; this is the negative case that keeps it honest.
#[test]
fn volatile_global_reads_are_never_served_from_the_w_cache() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/w_cache_volatile.c"
    );
    let (mut p, globals, asm) = compile_with_asm(fixture);
    // `in` is read twice and never written. Neither read may be served
    // from the cache, so both must name the global as their source.
    let in_addr = globals["in"];
    let in_reads = asm
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with(&format!("MOVFF 0x{in_addr:03X}, "))
                || l.starts_with(&format!("MOVF 0x{in_addr:03X},"))
        })
        .count();
    assert!(
        in_reads >= 2,
        "every volatile read of in must survive:\n{asm}"
    );
    p.run(400);
    let out = globals["out"];
    let slot = globals["slot"];
    assert_eq!(
        p.ram()[slot as usize],
        p.ram()[in_addr as usize],
        "slot mirrors the first read"
    );
    assert_eq!(
        p.ram()[out as usize],
        p.ram()[in_addr as usize],
        "out mirrors the second read"
    );
}

/// epic-cc#504 acceptance: a local aggregate with a constant initializer
/// whose address escapes (here through a function pointer) is copied in
/// from a clang-synthesized flash table. Above the copy-loop floor that
/// copy runs as one counted TBLRD loop, and the bytes it lands must be
/// exactly the initializer's.
///
/// Expected: 0x10 ^ 0x87 ^ 0x0F ^ (0x1234 & 0xFF) = 0x10 ^ 0x87 ^ 0x0F ^ 0x34.
#[test]
fn a_const_initialized_aggregate_copies_by_loop_and_reads_back_its_bytes() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/const_agg_loop.c"
    );
    let (mut p, globals, asm) = compile_with_asm(fixture);
    let main_body = asm
        .split("main:")
        .nth(1)
        .and_then(|rest| rest.split("RETURN").next())
        .expect("main body");
    assert_eq!(
        main_body.matches("TBLRD").count(),
        1,
        "the 18-byte copy must run as one loop, not 18 unrolled reads:\n{asm}"
    );
    assert!(
        main_body.contains("LFSR 1, 0x010"),
        "the loop seeds the destination pointer:\n{asm}"
    );
    p.run(400_000);
    let expected = 0x10u8 ^ 0x87 ^ 0x0F ^ 0x34;
    assert_eq!(
        p.ram()[globals["sink"] as usize],
        expected,
        "the loop must have landed the initializer's bytes intact"
    );
    assert!(p.halted());
}
