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
    check_const_stack(&cg, &asm);
    isel_pic_baseline::verify_page_fit(&m, &asm, &addrs);
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
    check_const_stack(&cg, &asm);
    isel_pic_baseline::verify_page_fit(&m, &asm, &addrs);
    let words = assemble_words(&device::PIC12F509, &asm);
    (
        PicBaseline::with_device(&device::PIC12F509, words),
        layout.globals,
        asm,
    )
}

/// The const-reader CALL costs a stack level the IR depth gate never
/// sees: with a read present, deepest frame plus `__start -> main` plus
/// the reader must fit the 2-level silicon stack (D-5). Mirrors the
/// driver's product check.
fn check_const_stack(cg: &callgraph::CallGraph, asm: &str) {
    if asm.contains("CALL __read_") {
        assert!(
            cg.max_depth + 1 <= device::PIC12F509.stack_depth as usize,
            "const reads need a __read CALL level the {}-level stack cannot take at call depth {}",
            device::PIC12F509.stack_depth,
            cg.max_depth
        );
    }
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

/// P3 pointer/array acceptance (docs/37 section 3 P3): a runtime RAM
/// pointer (FSR/INDF path) with a volatile index. `in` is a 16-bit
/// volatile so clang keeps the index mask as an i16 `and`. Expected:
/// in = 1 -> ram[1] = 2 -> out = 2.
#[test]
fn ptr_probe_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/ptr_probe.c");
    p.ram_mut()[globals["in"] as usize] = 1;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 2, "out = ram[1] = 2");
    // Pin the indexed cell: the store must land at ram[1], not ram[0] (an
    // index term dropped from the FSR setup would round-trip through ram[0]
    // and still read back into out == 2).
    assert_eq!(
        p.ram()[globals["ram"] as usize + 1],
        2,
        "store landed at ram[1]"
    );
    assert_eq!(
        p.ram()[globals["ram"] as usize],
        0,
        "ram[0] must stay untouched"
    );
    assert!(p.halted());
}

/// P3 array acceptance: a non-const array written and read at a runtime
/// index, the pure FSR/INDF path. Expected: in = 3 -> buf[3] = 4 -> out =
/// 4.
#[test]
fn array_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/array.c");
    p.ram_mut()[globals["in"] as usize] = 3;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 4, "out = buf[3] = 4");
    // Pin the indexed cell: buf[3], not buf[0] (a dropped index would
    // round-trip through buf[0] and still give out == 4).
    assert_eq!(
        p.ram()[globals["buf"] as usize + 3],
        4,
        "store landed at buf[3]"
    );
    assert_eq!(
        p.ram()[globals["buf"] as usize],
        0,
        "buf[0] must stay untouched"
    );
    assert!(p.halted());
}

/// P3 structs acceptance: byval calls (sum/pick) and a dynamic
/// array-in-struct through FSR/INDF. Expected: out == 0x48.
#[test]
fn structs_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile("tests/fixtures/structs.c");
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x48, "structs trace");
    // Pin the dynamic array cell: arr.v[2] must be 0x11, not arr.v[0] (a
    // dropped index from the store would round-trip through arr.v[0] and
    // pick() would still read it back into out).
    assert_eq!(
        p.ram()[globals["arr"] as usize + 3],
        0x11,
        "arr.v[2] = 0x11"
    );
    assert_eq!(
        p.ram()[globals["arr"] as usize + 1],
        0,
        "arr.v[0] untouched"
    );
    assert!(p.halted());
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

/// P4 const acceptance (docs/37 section 3 P4): a 128-byte flash table
/// read at a runtime index plus a direct scalar const read, served from
/// the page-0 low half through RETLW tables. Expected: in = 10 -> i =
/// 10, table[10] = 0x89, magic low byte 0x34 -> out = 0xBD.
#[test]
fn const_table_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/const_table.c");
    p.ram_mut()[globals["in"] as usize] = 10;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0xBD, "out = 0x89 + 0x34");
    assert!(p.halted());
    // The read goes through the table's reader entry in flash, not RAM:
    // a CALL into `__read_table` must be present, and no page-1 select
    // (the table shares page 0 with the code, D-5).
    assert!(
        asm.contains("CALL __read_table"),
        "const_table.c must CALL its RETLW reader:\n{asm}"
    );
    assert!(
        !asm.contains("BSF STATUS, 5"),
        "page-0 table needs no PA0 set:\n{asm}"
    );
    assert_eq!(
        ours_hex(&asm).trim(),
        gpasm_hex(&asm, "const_table").trim(),
        "our HEX differs from gpasm"
    );
}

/// P4 page-1 spill: code plus a 240-byte table exceeds the page-0 low
/// half, so the table relocates to the page-1 low half with PA0
/// set/restore around the read, while a second 100-byte table fits the
/// page-0 remainder (mixed-page emission). Expected: in = 150 -> i =
/// 134, big[134] = 0xD5, j = 6, small[6] = 0x55 -> out = 0x2A.
#[test]
fn const_spill_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/const_spill.c");
    p.ram_mut()[globals["in"] as usize] = 150;
    p.run(10_000);
    assert_eq!(p.ram()[globals["out"] as usize], 0x2A, "out = 0xD5 + 0x55");
    assert!(p.halted());
    // The big table spilled: an `org` past page 0 plus the PA0
    // set/restore around the cross-page CALL must be present, and both
    // readers must be called.
    assert!(
        asm.contains("org 0x200"),
        "spilled table must org into page 1:\n{asm}"
    );
    assert!(
        asm.contains("BSF STATUS, 5"),
        "page-1 read must set PA0:\n{asm}"
    );
    assert!(
        asm.contains("CALL __read_big"),
        "const_spill.c must CALL its spilled reader:\n{asm}"
    );
    assert!(
        asm.contains("CALL __read_small"),
        "const_spill.c must CALL its page-0 reader:\n{asm}"
    );
}

/// P4 ceiling regression: a 300-byte table fits no page low half (4-word
/// reader + 300 RETLWs > 256), so compilation panics loudly instead of
/// silently miscompiling (D-5).
#[test]
#[should_panic(expected = "too large")]
fn huge_const_table_is_rejected() {
    let _guard = E2E_LOCK.lock();
    let _ = compile("tests/fixtures/const_huge.c");
}

/// P4 stack-budget regression: a const read inside a callee nests
/// `__start -> main -> at` plus the reader CALL (3 levels) on the
/// 2-level stack, which drops the oldest return untrapped. Compilation
/// panics loudly instead of emitting it (D-5).
#[test]
#[should_panic(expected = "__read CALL level")]
fn deep_const_read_is_rejected() {
    let _guard = E2E_LOCK.lock();
    let _ = compile("tests/fixtures/const_deep.c");
}

/// Assemble `asm` with our encoder to HEX text for the gpasm comparison.
fn ours_hex(asm: &str) -> String {
    asm::assemble_file_to_hex(&device::PIC12F509, asm)
}

/// Assemble `asm` with gpasm (the oracle, GPL, never shipped) to HEX
/// text. Runs in the container like the `asm` crate's gpasm tests.
fn gpasm_hex(asm: &str, stem: &str) -> String {
    let gpasm = std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into());
    let dir = std::env::temp_dir().join(format!("pic_baseline_p4_{stem}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join(format!("{stem}.asm")), asm).expect("write asm");
    let out = Command::new(gpasm)
        .args([
            "-p",
            "p12f509",
            &format!("{stem}.asm"),
            "-o",
            &format!("{stem}.hex"),
        ])
        .current_dir(&dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(dir.join(format!("{stem}.hex"))).expect("read hex")
}

/// Parse Intel HEX data records into word address -> 12-bit word. Our
/// encoder emits dense images (gaps read back as zero); gpasm omits
/// unprogrammed ranges, so gapped programs compare on the intersection.
fn parse_hex_words(hex: &str) -> std::collections::HashMap<usize, u16> {
    let mut words = std::collections::HashMap::new();
    for line in hex.lines() {
        let line = line.trim();
        if !line.starts_with(':') {
            continue;
        }
        let n = usize::from_str_radix(&line[1..3], 16).expect("hex len");
        let addr = usize::from_str_radix(&line[3..7], 16).expect("hex addr");
        if &line[7..9] != "00" {
            continue;
        }
        for i in (0..n).step_by(2) {
            let lo = u16::from_str_radix(&line[9 + i * 2..11 + i * 2], 16).expect("hex byte");
            let hi = u16::from_str_radix(&line[11 + i * 2..13 + i * 2], 16).expect("hex byte");
            words.insert((addr + i) / 2, (hi << 8) | lo);
        }
    }
    words
}

/// Every word gpasm programs must match ours: the oracle cross-checks
/// encoding on all programmed words while ignoring gap fill/omission.
fn gpasm_agrees(asm: &str, stem: &str) {
    let ours = parse_hex_words(&ours_hex(asm));
    let theirs = parse_hex_words(&gpasm_hex(asm, stem));
    assert!(!theirs.is_empty(), "gpasm emitted no words for {stem}");
    for (addr, word) in &theirs {
        assert_eq!(
            ours.get(addr),
            Some(word),
            "word at {addr:#x} differs from gpasm"
        );
    }
}

/// P6 div-half acceptance (docs/37 section 3 P6): udiv/urem/sdiv/srem
/// on i16 and i8 through the software routine copies, laid out across
/// pages with PA0-managed CALLs. Expected: in = 301 -> out = 21.
#[test]
fn muldiv_div_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/muldiv_div.c");
    p.ram_mut()[globals["in"] as usize] = 45;
    p.ram_mut()[globals["in"] as usize + 1] = 1;
    p.run(2_000_000);
    assert_eq!(p.ram()[globals["out"] as usize], 21, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "muldiv_div");
}

/// P6 mul-half acceptance: mul plus const and variable-count shifts on
/// i16 and i8. Expected: in = 301 -> out = 616.
#[test]
fn muldiv_mul_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/muldiv_mul.c");
    p.ram_mut()[globals["in"] as usize] = 45;
    p.ram_mut()[globals["in"] as usize + 1] = 1;
    p.run(2_000_000);
    let out = p.ram()[globals["out"] as usize] as u16
        | ((p.ram()[globals["out"] as usize + 1] as u16) << 8);
    assert_eq!(out, 616, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "muldiv_mul");
}

/// P6 i32 add/mul acceptance (docs/37 section 3 P6): inline add,
/// mul_u32 (the only routine copy), const shifts and logic.
/// Expected: x = 0x12345678 -> 0xBF41AAC5.
#[test]
fn long_mul_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_mul.c");
    p.ram_mut()[globals["x"] as usize] = 0x78;
    p.ram_mut()[globals["x"] as usize + 1] = 0x56;
    p.ram_mut()[globals["x"] as usize + 2] = 0x34;
    p.ram_mut()[globals["x"] as usize + 3] = 0x12;
    p.run(2_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 0xBF41AAC5, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_mul");
}

/// P6 i32 unsigned div acceptance: udiv/urem through the software
/// routine copies. Expected: x = 0x12345678 -> 4.
#[test]
fn long_div_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_div.c");
    p.ram_mut()[globals["x"] as usize] = 0x78;
    p.ram_mut()[globals["x"] as usize + 1] = 0x56;
    p.ram_mut()[globals["x"] as usize + 2] = 0x34;
    p.ram_mut()[globals["x"] as usize + 3] = 0x12;
    p.run(8_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 4, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_div");
}

/// P6 i32 signed div acceptance: sdiv/srem through the software
/// routine copies, negative dividend and divisor.
/// Expected: x = -123456789 -> -4 (0xFFFFFFFC).
#[test]
fn long_sdiv_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_sdiv.c");
    p.ram_mut()[globals["x"] as usize] = 0xEB;
    p.ram_mut()[globals["x"] as usize + 1] = 0x32;
    p.ram_mut()[globals["x"] as usize + 2] = 0xA4;
    p.ram_mut()[globals["x"] as usize + 3] = 0xF8;
    p.run(8_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 0xFFFFFFFC, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_sdiv");
}

/// P6 i32 sub/compare acceptance: inline sub, unsigned compares and
/// const shifts. Expected: x = 0x12345678 -> 0x044CD55E.
#[test]
fn long_cmp_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_cmp.c");
    p.ram_mut()[globals["x"] as usize] = 0x78;
    p.ram_mut()[globals["x"] as usize + 1] = 0x56;
    p.ram_mut()[globals["x"] as usize + 2] = 0x34;
    p.ram_mut()[globals["x"] as usize + 3] = 0x12;
    p.run(2_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 0x044CD55E, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_cmp");
}

/// P6 i32 variable-shift acceptance: reg-count shifts through the
/// shl/lshr routine copies. Expected: x = 0x12345678, n = 3 ->
/// 0x12355779.
#[test]
fn long_shift_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_shift.c");
    p.ram_mut()[globals["x"] as usize] = 0x78;
    p.ram_mut()[globals["x"] as usize + 1] = 0x56;
    p.ram_mut()[globals["x"] as usize + 2] = 0x34;
    p.ram_mut()[globals["x"] as usize + 3] = 0x12;
    p.ram_mut()[globals["n"] as usize] = 3;
    p.run(2_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 0x12355779, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_shift");
}

/// P6 mul carry-fold regression (epic-cc#328 review): iteration 1
/// folds t = 0xFF with carry set at byte 1. Expected: 0x0000FFC0
/// -> 0x0002FF40 (pre-fix byte 2 reads 0x01).
#[test]
fn long_carry_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_carry.c");
    p.ram_mut()[globals["x"] as usize] = 0xC0;
    p.ram_mut()[globals["x"] as usize + 1] = 0xFF;
    p.ram_mut()[globals["x"] as usize + 2] = 0x00;
    p.ram_mut()[globals["x"] as usize + 3] = 0x00;
    p.run(2_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 0x0002FF40, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_carry");
}

/// P6 div borrow-fold regression: den 01 00 FF 00 wraps the byte-2
/// trial fold with borrow pending. Expected: 40000000 -> 2.
#[test]
fn long_divtrig_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_divtrig.c");
    p.ram_mut()[globals["x"] as usize] = 0x00;
    p.ram_mut()[globals["x"] as usize + 1] = 0x5A;
    p.ram_mut()[globals["x"] as usize + 2] = 0x62;
    p.ram_mut()[globals["x"] as usize + 3] = 0x02;
    p.run(8_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 2, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_divtrig");
}

/// P6 div borrow-fold regression, remainder-keep path.
/// Expected: 40000000 -> 6576638.
#[test]
fn long_remtrig_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_remtrig.c");
    p.ram_mut()[globals["x"] as usize] = 0x00;
    p.ram_mut()[globals["x"] as usize + 1] = 0x5A;
    p.ram_mut()[globals["x"] as usize + 2] = 0x62;
    p.ram_mut()[globals["x"] as usize + 3] = 0x02;
    p.run(8_000_000);
    let out = p.ram()[globals["x"] as usize] as u32
        | ((p.ram()[globals["x"] as usize + 1] as u32) << 8)
        | ((p.ram()[globals["x"] as usize + 2] as u32) << 16)
        | ((p.ram()[globals["x"] as usize + 3] as u32) << 24);
    assert_eq!(out, 6576638, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_remtrig");
}

/// P6 u16 div borrow-fold regression through the __scr den copy.
/// Pre-fix this yields 0x8080. Expected: x = 65535, y = 65282 -> 1.
#[test]
fn long_div16_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals, asm) = compile_asm("tests/fixtures/long_div16.c");
    p.ram_mut()[globals["x"] as usize] = 0xFF;
    p.ram_mut()[globals["x"] as usize + 1] = 0xFF;
    p.ram_mut()[globals["y"] as usize] = 0x02;
    p.ram_mut()[globals["y"] as usize + 1] = 0xFF;
    p.run(2_000_000);
    let out =
        p.ram()[globals["x"] as usize] as u16 | ((p.ram()[globals["x"] as usize + 1] as u16) << 8);
    assert_eq!(out, 1, "out trace");
    assert!(p.halted());
    gpasm_agrees(&asm, "long_div16");
}
