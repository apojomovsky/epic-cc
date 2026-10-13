//! Packed flag lowering (epic-cc#1162): `select_with_opts` with a bit
//! map lowers fused loads to in-place bit tests, const stores to
//! single-bit sets/clears, and remaining loads to masked 0/1 bytes.
//! Every test assembles and runs on the `Pic18` simulator with the
//! shared byte preset across bit patterns, so isolation (one flag
//! never disturbing its neighbor) is observed, not assumed.
use device::PIC18F4550;
use ir::parse;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(n, a)| (n.to_string(), *a)).collect()
}

fn packed(pairs: &[(&str, u8)]) -> HashMap<String, u8> {
    pairs.iter().map(|(n, b)| (n.to_string(), *b)).collect()
}

/// Instructions `__start` emits before `call main`: the shared counter
/// from the main suite plus packed `BSF`/`BCF` init lines, so a
/// set-initialized flag still parks the sim at main's entry.
fn start_steps(asm: &str) -> usize {
    let window = asm
        .split("__start:")
        .nth(1)
        .expect("__start label")
        .split("    call main")
        .next()
        .expect("__start must call main");
    let mut steps = 1;
    let mut zero_run = false;
    for line in window.lines() {
        let line = line.trim();
        if line.starts_with("LFSR") {
            zero_run = true;
            steps += 1;
        } else if let Some(take) = line.strip_prefix("MOVLW 0x") {
            if zero_run {
                steps += 3 * usize::from(u8::from_str_radix(take, 16).unwrap());
                zero_run = false;
            } else {
                steps += 1;
            }
        } else if line.starts_with("MOVWF")
            || line.starts_with("MOVLB")
            || line.starts_with("BSF")
            || line.starts_with("BCF")
        {
            steps += 1;
        }
    }
    steps
}

fn step_past_start(p: &mut pic14_sim::Pic18, steps: usize) {
    assert!(!p.halted());
    for _ in 0..steps {
        p.step();
    }
    assert!(!p.halted(), "__start window must not halt the machine");
}

fn select_packed(m: &ir::Module, addrs: &HashMap<String, u16>, bits: &[(&str, u8)]) -> String {
    isel_pic18::select_with_opts(
        &PIC18F4550,
        m,
        addrs,
        None,
        None,
        None,
        isel_pic18::Options::default(),
        &packed(bits),
    )
    .0
}

#[test]
fn fused_packed_branch_tests_the_bit_in_place() {
    // fa is bit 0 of 0x20, fb bit 1. The branch follows fa across all
    // four byte patterns: fb never leaks into the decision.
    let m = parse(
        "global fa i1 internal\nglobal fb i1 internal\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i1 @fa\n    br i1 %1 t f\n\
         block t:\n    store i8 1 @out\n    ret void\n\
         block f:\n    store i8 2 @out\n    ret void\n",
    );
    let addrs = addrs(&[("fa", 0x20), ("fb", 0x20), ("out", 0x21), ("main::1", 0x22)]);
    let asm = select_packed(&m, &addrs, &[("fa", 0), ("fb", 1)]);
    assert!(
        asm.contains("BTFSC 0x020, 0,A"),
        "fused bit test missing:\n{asm}"
    );
    assert!(
        !asm.contains("MOVFF 0x020"),
        "fused load must not copy the byte:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (byte, expect) in [(0x00u8, 2u8), (0x01, 1), (0x02, 2), (0x03, 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = byte;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x21],
            expect,
            "byte {byte:#04x} took the wrong edge"
        );
    }
}

#[test]
fn packed_const_stores_touch_only_their_bit() {
    // Preset 0x02 (fb set, fa clear): setting fa and clearing fb must
    // land exactly 0x01, proving BSF/BCF isolate their bits.
    let m = parse(
        "global fa i1 internal\nglobal fb i1 internal\n\
         fn main(void) ()\n  block entry:\n    store i1 1 @fa\n    store i1 0 @fb\n    ret void\n",
    );
    let addrs = addrs(&[("fa", 0x20), ("fb", 0x20)]);
    let asm = select_packed(&m, &addrs, &[("fa", 0), ("fb", 1)]);
    assert!(asm.contains("BSF 0x020, 0,A"), "set bit missing:\n{asm}");
    assert!(asm.contains("BCF 0x020, 1,A"), "clear bit missing:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x20] = 0x02;
    p.run(200);
    assert!(p.halted());
    assert_eq!(p.ram()[0x20], 0x01, "stores must isolate their bits");
}

#[test]
fn materialized_packed_load_masks_to_01() {
    // The load feeds two uses, so it materializes: the slot reads fb's
    // bit only, across all four byte patterns.
    let m = parse(
        "global fa i1 internal\nglobal fb i1 internal\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i1 @fb\n    %2 = zext i1 %1 to i8\n\
           store i8 %2 @out\n    br i1 %1 t f\n\
         block t:\n    ret void\n\
         block f:\n    ret void\n",
    );
    let addrs = addrs(&[
        ("fa", 0x20),
        ("fb", 0x20),
        ("out", 0x21),
        ("main::1", 0x22),
        ("main::2", 0x23),
    ]);
    let asm = select_packed(&m, &addrs, &[("fa", 0), ("fb", 1)]);
    assert!(
        asm.contains("BTFSC 0x020, 1,A"),
        "masked bit read missing:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (byte, expect) in [(0x00u8, 0u8), (0x01, 0), (0x02, 1), (0x03, 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = byte;
        p.run(200);
        assert!(p.halted());
        assert_eq!(p.ram()[0x21], expect, "byte {byte:#04x} read wrong");
    }
}

#[test]
fn single_use_zext_fold_reads_the_slot_not_the_byte() {
    // A single-use load into a zext matches the load-zext fold, but a
    // packed source must still materialize: the zext reads the masked
    // slot, never the shared byte.
    let m = parse(
        "global fa i1 internal\nglobal fb i1 internal\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i1 @fb\n    %2 = zext i1 %1 to i8\n\
           store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("fa", 0x20),
        ("fb", 0x20),
        ("out", 0x21),
        ("main::1", 0x22),
        ("main::2", 0x23),
    ]);
    let asm = select_packed(&m, &addrs, &[("fa", 0), ("fb", 1)]);
    let words = asm::assemble_pic18(&asm);
    for (byte, expect) in [(0x00u8, 0u8), (0x01, 0), (0x02, 1), (0x03, 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = byte;
        p.run(200);
        assert!(p.halted());
        assert_eq!(p.ram()[0x21], expect, "byte {byte:#04x} read wrong");
    }
}

#[test]
fn cross_bank_materialized_load_stays_skip_safe() {
    // Slot and shared byte in different banks: a bank select must never
    // land between the bit test and its skipped instruction, or the set
    // hits the wrong bank and corrupts RAM.
    let m = parse(
        "global fa i1 internal\nglobal fb i1 internal\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i1 @fb\n    %2 = zext i1 %1 to i8\n\
           store i8 %2 @out\n    br i1 %1 t f\n\
         block t:\n    ret void\n\
         block f:\n    ret void\n",
    );
    let addrs = addrs(&[
        ("fa", 0x120),
        ("fb", 0x120),
        ("out", 0x121),
        ("main::1", 0x220),
        ("main::2", 0x221),
    ]);
    let asm = select_packed(&m, &addrs, &[("fa", 0), ("fb", 1)]);
    let words = asm::assemble_pic18(&asm);
    for (byte, expect) in [(0x00u8, 0u8), (0x01, 0), (0x02, 1), (0x03, 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x120] = byte;
        p.run(200);
        assert!(p.halted());
        assert_eq!(p.ram()[0x121], expect, "byte {byte:#04x} read wrong");
        assert_eq!(
            p.ram()[0x120],
            byte,
            "shared byte must survive, byte {byte:#04x}"
        );
    }
}
