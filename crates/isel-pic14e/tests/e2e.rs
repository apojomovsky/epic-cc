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
    let (p, globals, asm, _locals) = compile_with_layout(c_path);
    (p, globals, asm)
}

/// Like `compile`, but also returns the emitted `.asm` text and the full
/// locals slot map so a test can assert the runtime routine frames.
fn compile_with_layout(
    c_path: &str,
) -> (Pic14e, HashMap<String, u16>, String, HashMap<String, u16>) {
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

    (
        Pic14e::with_device(&PIC16F1937, words),
        layout.globals,
        asm,
        layout.locals,
    )
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

// P3/P4 end-to-end acceptance (docs/33 section 4): pointers, arrays and
// structs via FSR0/1, plus linear addressing (D-2) for objects that
// straddle a bank. The fixtures are byte-identical to the PIC18 ones of
// the same C source (crates/isel-pic18/tests/fixtures/), so the expected
// values come from the PIC18 e2e tests. `ptr_probe.c` is the original
// Milestone-5 probe: a runtime RAM pointer (FSR/INDF) AND a const-table
// read (RETLW), both in one program (P4 restores the const-flash read).

#[test]
fn ptr_probe_c_runs_correctly() {
    // in = 1; i = 1 & 3 = 1; ram[1] = table[1] = 20; out = ram[1] = 20.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ptr_probe.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 0x01; // in low byte
    p.ram_mut()[in_addr + 1] = 0x00; // in high byte
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        20,
        "out == table[1] == 20 read through the pointer"
    );
    assert!(p.halted());
}

/// P4 acceptance (docs/33 section 4, D-5): a 300-byte const (flash) table
/// read through the two-entry chunked RETLW readers. Mirrors
/// crates/driver/tests/const_table_e2e.rs: in == 290 (0x0122) -> out =
/// (0x33 + 0x02 + 0x3C + 0x11) & 0xFF = 0x82, the four reads exercising
/// chunk-1, chunk-0, chunk-1-last, and chunk-boundary byte offsets. The
/// 511-byte ceiling the RETLW mechanism imposes stays (D-5).
#[test]
fn const_table_c_runs_correctly() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/const_table.c"
    ));
    let in_addr = globals["in"] as usize;
    p.ram_mut()[in_addr] = 0x22; // 290 = 0x0122, lo byte
    p.ram_mut()[in_addr + 1] = 0x01; // hi byte
    p.run(500_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x82,
        "out == 0x82 for in == 290 (four boundary reads)"
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

/// P3 regression (reviewer finding): a runtime pointer VALUE to a
/// bank-straddling global must carry the linear alias, so a deref at an
/// offset past the bank boundary walks the linear region (which compresses
/// the common-RAM hole), not the hole itself. The pointer is passed through
/// a function boundary so it is genuinely runtime, not a static GEP.
#[test]
fn span_ptr_c_runtime_pointer_to_straddling_global_uses_linear_base() {
    let (mut p, globals, asm) = compile_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/span_ptr.c"
    ));
    let big = globals["big"];
    assert!(
        big >= 0x20 && big <= 0x6F && 0x6F - big + 1 < 90,
        "span_ptr.c layout: big[90] at 0x{big:03X} must straddle bank 0 -> bank 1"
    );
    // The pointer value stored to `gp` and passed to `sum` must be the
    // linear alias (0x2000 + bank*80 + (off-0x20)), not the physical base,
    // so a deref at offset 89 walks the linear region (which compresses the
    // common-RAM hole), not the hole itself.
    let linear_base = 0x2000 + (big & 0x7F) - 0x20;
    assert!(
        (0x2000..=0x29AF).contains(&linear_base),
        "big's linear base 0x{linear_base:04X} must be in the linear region"
    );
    // The asm must materialize the pointer value as the linear base at the
    // `gp` store site: the low byte stored to gp is the linear base's low
    // byte (0x00) and the high byte is its high byte (0x20). The physical
    // base 0x{big:02X} would store low 0x{big:02X} / high 0x00 instead, so
    // this distinguishes the two. The store uses the banked file-register
    // form (low 7 bits of the physical address, with a preceding MOVLB).
    let lo = (linear_base & 0xFF) as u8;
    let hi = ((linear_base >> 8) & 0xFF) as u8;
    let gp = globals["gp"];
    let gp_lo = format!("MOVWF 0x{:02X}", gp & 0x7F);
    let gp_hi = format!("MOVWF 0x{:02X}", (gp + 1) & 0x7F);
    let lines: Vec<&str> = asm.lines().map(|l| l.trim()).collect();
    // The MOVLW that feeds a given MOVWF is the nearest preceding MOVLW
    // (a MOVLB may sit between them).
    let feeding_movlw = |store: &str| {
        let idx = lines.iter().position(|l| *l == store)?;
        lines[..idx]
            .iter()
            .rev()
            .find(|l| l.starts_with("MOVLW "))
            .map(|l| l.to_string())
    };
    let lo_ok = feeding_movlw(&gp_lo) == Some(format!("MOVLW 0x{lo:02X}"));
    let hi_ok = feeding_movlw(&gp_hi) == Some(format!("MOVLW 0x{hi:02X}"));
    assert!(
        lo_ok && hi_ok,
        "span_ptr.c must store the linear base 0x{linear_base:04X} to gp \
         (low 0x{lo:02X} -> {gp_lo}, high 0x{hi:02X} -> {gp_hi}):\n{asm}"
    );
    // And the simulated result must be right: big[0] + big[89] = 0x11 + 0x22.
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x33,
        "out == big[0] + big[89] == 0x11 + 0x22 through a runtime pointer"
    );
    assert!(p.halted());
}

/// P3 regression (reviewer finding): a constant-length memcpy whose
/// destination is a bank-straddling global must route through FSR0 with the
/// linear base, not emit direct file-register stores that walk into the
/// common-RAM hole and bank-1 SFRs.
#[test]
fn span_memcpy_c_into_straddling_destination_uses_linear_base() {
    let (mut p, globals, asm) = compile_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/span_memcpy.c"
    ));
    let big = globals["big"];
    assert!(
        big >= 0xA0 && big <= 0xEF && 0xEF - big + 1 < 90,
        "span_memcpy.c layout: big[90] at 0x{big:03X} must straddle bank 1 -> bank 2"
    );
    // The memcpy destination must be addressed through the linear region:
    // the asm must contain a MOVLW of the linear base's high byte (0x20)
    // before a MOVWF FSR0H, and the base's low byte before a MOVWF FSR0L.
    let linear_base = 0x2000 + (big & 0x7F) - 0x20;
    let lo = (linear_base & 0xFF) as u8;
    let hi = ((linear_base >> 8) & 0xFF) as u8;
    let lines: Vec<&str> = asm.lines().map(|l| l.trim()).collect();
    let has_lo = lines.iter().any(|l| *l == format!("MOVLW 0x{lo:02X}"));
    let has_hi = lines.iter().any(|l| *l == format!("MOVLW 0x{hi:02X}"));
    assert!(
        has_lo && has_hi,
        "span_memcpy.c must address the straddling destination through the \
         linear base 0x{linear_base:04X} (MOVLW 0x{lo:02X} / MOVLW 0x{hi:02X}):\n{asm}"
    );
    // And the simulated result must be right: big[79] == src[79] == 0x50.
    p.run(200_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x50,
        "out == big[79] == src[79] == 0x50 after the 80-byte memcpy"
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

// P5 end-to-end acceptance (docs/33 section 4, D-4): interrupts. The
// fixtures are byte-identical to PIC14's except for the SFR addresses
// (PORTB 0x06 -> 0x0D on the 1937; INTCON stays 0x0B), so the expected
// values come from the PIC14 e2e tests of the same C source
// (crates/driver/tests/{interrupt,interrupt_gate}_e2e.rs).

#[test]
fn interrupt_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    // Mirrors crates/driver/tests/interrupt_e2e.rs: in == 0x10, the ISR
    // fired mid-run right after main's PORTB = 0x11 store -> the ISR's
    // bump_isr(out) lands before main's bump reads it:
    //   out = 0x10 -> ISR bumps to 0x11 -> main: bump(0x11)=0x12 -> +1
    //   = 0x13 -> +bump(2)=3 -> 0x16; PORTB ends 0x22.
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt.c"
    ));
    let in_addr = globals["in"] as usize;
    let out_addr = globals["out"] as usize;
    p.ram_mut()[in_addr] = 0x10;

    // Run main to the injection point: right after the `PORTB = 0x11`
    // store (detected by PORTB's value rather than a fixed word count,
    // since the PIC14E layout is instruction-denser than classic PIC14's).
    let mut steps = 0usize;
    while p.ram()[0x0D] != 0x11 {
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

    // Fire the interrupt: the hardware saves W/STATUS/BSR/FSR/PCLATH to
    // its shadow registers, pushes the return PC and jumps to the vector
    // 0x0004.
    p.fire_interrupt();
    assert_eq!(p.pc(), 0x0004, "the ISR starts at the vector");

    // The ISR runs (PORTB = 0x55, out = bump_isr(out)), RETFIE restores
    // the shadow context and returns to the interrupted instruction, and
    // main completes: out == 0x16, PORTB == 0x22, then the __start SLEEP
    // halts the machine.
    p.run(500_000);
    assert_eq!(
        p.ram()[out_addr],
        0x16,
        "out == hand-computed 0x16 (ISR bump 0x10 -> 0x11, then 0x11 -> 0x12 -> 0x13 -> 0x16)"
    );
    assert_eq!(
        p.ram()[0x0D],
        0x22,
        "PORTB == 0x22 (main's final SFR write)"
    );
    assert!(p.halted());
}

/// P5 acceptance (docs/33 section 4, D-4): the ISR must be emitted with NO
/// manual save/restore prologue, because the hardware shadow registers save
/// W, STATUS, BSR, FSR0, FSR1 and PCLATH on entry and restore them on
/// RETFIE. A simulation pass alone cannot distinguish "the hardware saved
/// it" from "we saved it manually and it happened to work", so this static
/// assertion on the emitted `.asm` is the point of the phase's acceptance.
/// The old manual prologue used `SWAPF 0x75, F` / `SWAPF STATUS, W` /
/// `MOVF PCLATH, W` / `MOVF FSR, W`; none of those may appear, and a
/// `RETFIE` must close the handler.
#[test]
fn interrupt_c_emits_no_manual_context_save() {
    let (_p, _globals, asm) = compile_asm(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt.c"
    ));
    let lines: Vec<&str> = asm.lines().map(|l| l.trim()).collect();
    let manual_save = [
        "SWAPF 0x75, F",
        "SWAPF STATUS, W",
        "SWAPF 0x76, W",
        "SWAPF 0x75, W",
    ];
    for idiom in manual_save {
        assert!(
            !lines.iter().any(|l| *l == idiom),
            "interrupt.c must not emit the manual context-save idiom `{idiom}` \
             (the hardware shadow registers save W/STATUS/BSR/FSR/PCLATH, D-4):\n{asm}"
        );
    }
    assert!(
        !lines
            .iter()
            .any(|l| *l == "MOVF PCLATH, W" || *l == "MOVF FSR, W"),
        "interrupt.c must not read back PCLATH/FSR to save them (the hardware \
         shadows them, D-4):\n{asm}"
    );
    assert!(
        lines.iter().any(|l| *l == "RETFIE"),
        "interrupt.c must close the ISR with RETFIE (the hardware restores \
         the shadow context there):\n{asm}"
    );
}

/// P5 acceptance (docs/33 section 4, D-4), the gating path: a request made
/// while GIE is clear stays pending; it is taken only once main unmasks,
/// and exactly once. Mirrors crates/driver/tests/interrupt_gate_e2e.rs.
#[test]
fn interrupt_gate_c_runs_correctly() {
    let _guard = E2E_LOCK.lock();
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt_gate.c"
    ));
    let stage = globals["stage"] as usize;
    let isr_ran = globals["isr_ran"] as usize;

    // Run into the masked window (stage == 1) and request the interrupt.
    let mut steps = 0usize;
    while p.ram()[stage] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 1000, "never reached stage 1 (pc = {})", p.pc());
    }
    p.request_interrupt();
    assert!(p.interrupt_pending(), "the request latches");

    // Through the whole masked window the handler must not run.
    steps = 0;
    while p.ram()[stage] != 2 {
        p.step();
        steps += 1;
        assert!(steps < 1000, "never reached stage 2 (pc = {})", p.pc());
    }
    assert_eq!(
        p.ram()[isr_ran],
        0,
        "the masked window must not run the ISR"
    );

    // Main sets GIE; the pending request is taken, once.
    p.run(500_000);
    assert_eq!(p.ram()[isr_ran], 1, "the ISR ran exactly once after GIE");
    assert_eq!(p.ram()[stage], 3, "main reaches its final stage");
    assert!(p.halted());
}

// P6 end-to-end acceptance (docs/33 section 4): i32 (`long`) arithmetic,
// hardware-multiply routine recipes, and the ISR-context routine
// duplication. Fixtures are byte-identical to PIC14's; expected values come
// from the PIC14 e2e tests of the same C source
// (crates/driver/tests/{long,muldiv,interrupt_mul}_e2e.rs).

#[test]
fn long_c_runs_correctly() {
    // Mirrors crates/driver/tests/long_e2e.rs: in = 0x12345678, sin = -19
    // -> out = 0x1634943A (the whole i32 surface: add/mul/udiv/urem/sdiv/
    // srem/shifts/icmps/casts/struct-byval-sret).
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/long.c"
    ));
    p.ram_mut()[globals["in"] as usize] = 0x78; // 0x12345678
    p.ram_mut()[globals["in"] as usize + 1] = 0x56;
    p.ram_mut()[globals["in"] as usize + 2] = 0x34;
    p.ram_mut()[globals["in"] as usize + 3] = 0x12;
    p.ram_mut()[globals["sin"] as usize] = 0xED; // -19 = 0xFFFFFFED
    p.ram_mut()[globals["sin"] as usize + 1] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 2] = 0xFF;
    p.ram_mut()[globals["sin"] as usize + 3] = 0xFF;
    p.run(2_000_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        0x3A,
        "out == 0x1634943A, byte 0"
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
    p.ram_mut()[globals["in"] as usize] = 0x2D; // 301 = 0x012D, lo byte
    p.ram_mut()[globals["in"] as usize + 1] = 0x01; // hi byte
    p.run(500_000);
    assert_eq!(
        p.ram()[globals["out"] as usize],
        210,
        "out == hand-computed 210"
    );
    assert!(p.halted());
}

#[test]
fn interrupt_mul_c_runs_correctly() {
    // Mirrors crates/driver/tests/interrupt_mul_e2e.rs: main and the ISR
    // both multiply/divide, so both contexts reach the injected __mul_u8
    // and __udiv_u8 routines; the _isr copies must have disjoint frames,
    // which is what makes a mid-routine clobber impossible.
    let (mut p, globals, asm, locals) = compile_with_layout(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interrupt_mul.c"
    ));
    p.ram_mut()[globals["in_a"] as usize] = 47;
    p.ram_mut()[globals["in_b"] as usize] = 5;
    p.ram_mut()[globals["isr_a"] as usize] = 0xAB;
    p.ram_mut()[globals["isr_b"] as usize] = 3;
    p.run(1_000_000);
    // main's context: 47 * 5 = 235 (0xEB), 47 / (5|1) = 47/5 = 9.
    assert_eq!(p.ram()[globals["out"] as usize], 235, "main mul");
    assert_eq!(p.ram()[globals["out_q"] as usize], 9, "main div");

    // The ISR context must get its own routine copies with disjoint frames:
    // an interrupt taken while main is partway through __mul_u8 must not
    // re-enter the same frame (the bug issue #2 pinned). Both copies exist
    // in the alloc layout, and no byte of main's frame overlaps the ISR's.
    let slot_of = |f: &str, name: &str| {
        *locals
            .get(&format!("{f}::{name}"))
            .unwrap_or_else(|| panic!("no slot {f}::{name} in the layout"))
    };
    assert!(
        locals.contains_key("__mul_u8_isr::__scr"),
        "the ISR context must get its own __mul_u8_isr copy"
    );
    // __mul_u8's frame: params a/b + the 6-byte __scr (legalize's
    // routine_func sizing); same slot set for the _isr copy.
    let frame = |f: &str| {
        let (a, b, scr) = (slot_of(f, "a"), slot_of(f, "b"), slot_of(f, "__scr"));
        [(a, 1), (b, 1), (scr, 6)]
    };
    for (ma, msz) in frame("__mul_u8") {
        for (ia, isz) in frame("__mul_u8_isr") {
            let overlap = ma < ia + isz && ia < ma + msz;
            assert!(
                !overlap,
                "__mul_u8 frame [0x{ma:02X},+{msz}) overlaps __mul_u8_isr \
                 [0x{ia:02X},+{isz}) - an interrupt during main's multiply \
                 would clobber it"
            );
        }
    }
    // Both routine bodies emit, and the ISR's call targets its own copy.
    assert!(
        asm.lines().any(|l| l.trim() == "__mul_u8:"),
        "main's mul body:\n{asm}"
    );
    assert!(
        asm.lines().any(|l| l.trim() == "__mul_u8_isr:"),
        "the ISR mul copy must emit its own body:\n{asm}"
    );
    assert!(
        asm.lines().any(|l| l.trim() == "CALL __mul_u8_isr"),
        "the ISR must call its own __mul_u8_isr copy:\n{asm}"
    );
    assert!(p.halted());
}
