use device::PIC16F877A;
use ir::parse;
use isel::{select, verify_page_fit};
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn emits_add_for_in_plus_one() {
    // Milestone 3: locals come from the map too, keyed `{func}::{name}`.
    // alloc: globals in=0x20/out=0x21 -> end_of_globals 0x22 -> the root
    // frame starts at 0x25, so main's locals land at 0x25/0x26.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(asm.contains("MOVF 0x20, W"));
    assert!(
        asm.contains("MOVWF 0x25"),
        "%1 must live at its map address 0x25:\n{asm}"
    );
    assert!(asm.contains("ADDLW 0x01"));
    assert!(
        asm.contains("MOVWF 0x26"),
        "%2 must live at its map address 0x26:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x21"));
}

#[test]
fn store_const_emits_movlw_not_movf() {
    let m = parse(
        "global out i8\nfn main(void) ()\n  block entry:\n    store i8 5 @out\n    ret void\n",
    );
    let mut addrs = HashMap::new();
    addrs.insert("out".to_string(), 0x21u16);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW 0x05"),
        "expected MOVLW for const store:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x21"), "expected MOVWF to @out:\n{asm}");
    assert!(
        !asm.contains("MOVF 0x05"),
        "const must not be read as a file register:\n{asm}"
    );
}

#[test]
fn add_const_lhs_uses_addlw() {
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %x = add i8 5, %1\n    store i8 %x @out\n    ret void\n");
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::x", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("ADDLW 0x05"),
        "const-LHS add should use the ADDLW path:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x26"),
        "the result lands at its map address:\n{asm}"
    );
    assert!(
        !asm.contains("ADDWF 0x05"),
        "const must not be read as a file register:\n{asm}"
    );
    assert!(
        !asm.contains("MOVF 0x05"),
        "const must not be read as a file register:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "only i8/i16 loads supported")]
fn panics_on_i1_load() {
    let m = parse(
        "global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i1 @in\n    ret void\n",
    );
    select(&PIC16F877A, &m, &HashMap::new());
}

#[test]
#[should_panic(expected = "no slot for main::1")]
fn panics_when_local_address_missing_from_map() {
    // Every local address comes from the map; a missing entry must fail
    // loudly instead of allocating a slot internally.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn add16_reg_reg_emits_carry_chain() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %r = add i16 %a, %b\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2B,
    // %r=0x2D in IR order.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::r", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x29/%b=0x2B/%r=0x2D (lo bytes): lo byte add then hi byte add with carry in.
    assert!(asm.contains("MOVF 0x2B, W"), "add b_lo:\n{asm}");
    assert!(asm.contains("ADDWF 0x29, W"), "add a_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x2D"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVF 0x2C, W"), "add b_hi:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 0"), "carry test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "carry in add:\n{asm}");
    assert!(asm.contains("ADDWF 0x2A, W"), "add a_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x2E"), "store d_hi:\n{asm}");
}

#[test]
fn add16_reg_const_emits_carry_chain() {
    // 515 = 0x0203 -> lo 0x03, hi 0x02 (hi differs from the carry ADDLW 0x01,
    // so the k_hi add line is distinguishable).
    let m = parse(
        "global in i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @in\n    %r = add i16 %a, 515\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x24 -> root frame at 0x27; %a=0x27, %r=0x29.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::a", 0x27),
        ("main::r", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x27/%r=0x29.
    assert!(asm.contains("MOVF 0x27, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("ADDLW 0x03"), "add k_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x29"), "store d_lo:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 0"), "carry test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "carry in add:\n{asm}");
    assert!(asm.contains("ADDLW 0x02"), "add k_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store d_hi:\n{asm}");
}

#[test]
fn i16_local_uses_consecutive_map_addresses() {
    // The map is authoritative: an i16 local occupies the two consecutive
    // bytes at its map address (lo, lo+1). alloc: globals g0..g15 fill
    // 0x20..0x2F, out (i16) at 0x30 -> end_of_globals 0x32 -> root frame at
    // 0x35; main's 16 i8 locals fill 0x35..0x44 and the i16 %r lands at
    // 0x45/0x46, all inside bank 0 (no straddle into 0x80).
    let globals: String = (0..16).map(|i| format!("global g{i} i8\n")).collect();
    let loads: String = (0..16)
        .map(|i| format!("    %a{i} = load i8 @g{i}\n"))
        .collect();
    let m = parse(&format!(
        "{globals}global out i16\nfn main(void) ()\n  block entry:\n{loads}    %r = load i16 @out\n    store i16 %r @out\n    ret void\n"
    ));
    let mut addrs: HashMap<String, u16> = (0..16).map(|i| (format!("g{i}"), 0x20 + i)).collect();
    addrs.insert("out".to_string(), 0x30u16);
    for i in 0..16 {
        addrs.insert(format!("main::a{i}"), 0x35 + i);
    }
    addrs.insert("main::r".to_string(), 0x45u16);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVWF 0x45"),
        "i16 lo should land at map address 0x45:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x46"),
        "i16 hi should land at map address 0x46:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x46, W"),
        "store reads the i16 hi from 0x46:\n{asm}"
    );
}

#[test]
fn and16_reg_const_uses_andlw() {
    // 4660 = 0x1234 -> lo 0x34, hi 0x12.
    let m = parse(
        "global in i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @in\n    %r = and i16 %a, 4660\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: root frame at 0x27; %a=0x27, %r=0x29.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::a", 0x27),
        ("main::r", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x27/%r=0x29.
    assert!(asm.contains("MOVF 0x27, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("ANDLW 0x34"), "and k_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x29"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVF 0x28, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("ANDLW 0x12"), "and k_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store d_hi:\n{asm}");
}

#[test]
fn zext_trunc_pair() {
    let m = parse(
        "global in i8\nglobal out16 i16\nglobal out8 i8\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %z = zext i8 %v to i16\n    store i16 %z @out16\n    %t = trunc i16 %z to i8\n    store i8 %t @out8\n    ret void\n",
    );
    // alloc: in=0x20, out16 (i16, even-aligned)=0x22, out8=0x24 ->
    // end_of_globals 0x24 -> root frame at 0x27; %v=0x27, %z=0x28, %t=0x2A.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out16", 0x22),
        ("out8", 0x24),
        ("main::v", 0x27),
        ("main::z", 0x28),
        ("main::t", 0x2A),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %v=0x27, %z lo=0x28 hi=0x29, %t=0x2A. epic-cc#214: %v's own reload
    // for the zext is redundant (still in W right after Inst::Load's own
    // store to 0x27) and gets elided.
    assert!(asm.contains("MOVWF 0x28"), "zext stores d_lo:\n{asm}");
    assert!(asm.contains("CLRF 0x29"), "zext zeroes d_hi:\n{asm}");
    assert!(asm.contains("MOVF 0x28, W"), "trunc reads z_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "trunc stores d:\n{asm}");
}

#[test]
fn zext_i1_to_i8_copies_the_icmp_byte() {
    // `zext i1 %c to i8` is legal and common (`u8 b = (a < b);`): i1 and
    // i8 are both 1 byte in the byte model, and an icmp result is
    // materialized as a byte holding exactly 0/1, so the zext is a 1-byte
    // copy (equal-width zext identity). Pins the i1->i8 path independently
    // of the fuzz corpus.
    let m = parse(
        "global x i8\nglobal y i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %c = icmp eq i8 %a, %b\n    %z = zext i1 %c to i8\n    store i8 %z @out\n    ret void\n",
    );
    // alloc: globals end at 0x23 -> root frame at 0x26; %a=0x26, %b=0x27,
    // %c=0x28, %z=0x29.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("out", 0x22),
        ("main::a", 0x26),
        ("main::b", 0x27),
        ("main::c", 0x28),
        ("main::z", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The zext copies the icmp result byte into %z: MOVF 0x28, W; MOVWF 0x29.
    assert!(asm.contains("MOVF 0x28, W"), "load the icmp byte:\n{asm}");
    assert!(
        asm.contains("MOVWF 0x29"),
        "copy into %z (i1 is 1 byte, so the zext IS the copy):\n{asm}"
    );
}

#[test]
fn sext_i8_to_i16_copies_low_and_sign_fills_high() {
    let m = parse(
        "global in i8\nglobal out16 i16\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %s = sext i8 %v to i16\n    store i16 %s @out16\n    ret void\n",
    );
    // alloc: in=0x20, out16 (i16, even-aligned)=0x22 -> end_of_globals 0x24
    // -> root frame at 0x27; %v=0x27, %s=0x28 (hi 0x29).
    let addrs = addrs(&[
        ("in", 0x20),
        ("out16", 0x22),
        ("main::v", 0x27),
        ("main::s", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %v=0x27, %s lo=0x28 hi=0x29. epic-cc#214: %v's own reload for the
    // sext is redundant (still in W right after Inst::Load's own store to
    // 0x27) and gets elided.
    assert!(asm.contains("MOVWF 0x28"), "sext stores d_lo:\n{asm}");
    // Sign-fill: test the source's MSB (byte 0 of the i8 lives at 0x27),
    // then fill the high byte with 0xFF (negative) or 0x00 (positive).
    assert!(
        asm.contains("BTFSS 0x27, 7"),
        "sext tests v's sign bit:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0xFF"),
        "sext fills 0xFF when negative:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00"),
        "sext fills 0x00 when positive:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x29"), "sext stores d_hi:\n{asm}");
}

#[test]
fn sext_i8_to_i16_simulates_sign_extension() {
    use pic14_sim::Pic14;
    let m = parse(
        "global in i8\nglobal out i16\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %s = sext i8 %v to i16\n    store i16 %s @out\n    ret void\n",
    );
    let map = addrs(&[
        ("in", 0x20u16),
        ("out", 0x22),
        ("main::v", 0x27),
        ("main::s", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &map);
    let words = asm::assemble(&asm);
    // -1 -> 0xFFFF.
    {
        let mut p = Pic14::new(words.clone());
        p.ram_mut()[0x20] = 0xFF;
        p.run(200_000);
        assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
        assert_eq!(p.ram()[0x22], 0xFF, "sext(-1) lo byte");
        assert_eq!(p.ram()[0x23], 0xFF, "sext(-1) hi byte");
    }
    // +1 -> 0x0001.
    {
        let mut p = Pic14::new(words);
        p.ram_mut()[0x20] = 0x01;
        p.run(200_000);
        assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
        assert_eq!(p.ram()[0x22], 0x01, "sext(+1) lo byte");
        assert_eq!(p.ram()[0x23], 0x00, "sext(+1) hi byte");
    }
}

#[test]
fn phi_copy_lands_before_terminator_of_each_predecessor() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    br merge\n  block thenb:\n    %b = load i16 @y\n    br merge\n  block merge:\n    %p = phi i16 %a entry %b thenb\n    store i16 %p @out\n    ret void\n",
    );
    // alloc: root frame at 0x29; %a=0x29, %b=0x2B, %p=0x2D in IR order.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::p", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x29 (hi 0x2A), %b=0x2B (hi 0x2C), %p=0x2D (hi 0x2E).
    // In block `entry` the copy of %a (ending MOVWF 0x2E) precedes its GOTO.
    assert!(
        asm.contains("MOVWF 0x2E\n    GOTO main_Lmerge"),
        "copy must land before the entry terminator:\n{asm}"
    );
    // In block `thenb` the copy of %b (ending MOVWF 0x2E) precedes its GOTO.
    assert!(
        asm.contains(
            "MOVF 0x2B, W\n    MOVWF 0x2D\n    MOVF 0x2C, W\n    MOVWF 0x2E\n    GOTO main_Lmerge"
        ),
        "copy must land before the thenb terminator:\n{asm}"
    );
    // The merge block reads the phi destination (0x2D lo / 0x2E hi).
    assert!(asm.contains("MOVF 0x2D, W"), "merge reads %p lo:\n{asm}");
    assert!(
        asm.contains("MOVWF 0x24"),
        "merge stores %p lo to @out:\n{asm}"
    );
}

#[test]
fn phi_copy_chain_emits_dependent_copies_in_order() {
    // Two phis in the same merge block where one feeds the other: %p <- %a
    // and %q <- %p. The %q copy reads %p's slot, so isel must emit %p's copy
    // before %q's — never the reverse, which would clobber %a's value in
    // flight.
    let m = parse(
        "global x i8\nglobal y i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    br merge\n  block merge:\n    %p = phi i8 %a entry\n    %q = phi i8 %p entry\n    store i8 %q @out\n    ret void\n",
    );
    // alloc: root frame at 0x26; %a=0x26, %p=0x27, %q=0x28 (IR order).
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("out", 0x22),
        ("main::a", 0x26),
        ("main::p", 0x27),
        ("main::q", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %p=0x27, %q=0x28, %a=0x26. epic-cc#214: %q's own reload of %p's slot
    // is redundant (still in W right after %p's own copy stores it there)
    // and gets elided; %p's copy still strictly precedes %q's, the actual
    // property this test checks.
    assert!(
        asm.contains("MOVF 0x26, W\n    MOVWF 0x27\n    MOVWF 0x28"),
        "dependent copies must emit %p before %q:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "cyclic phi copies")]
fn panics_on_cyclic_phi_copies() {
    // A swap carried across a loop (clang -O1): %p = phi [%q], %q = phi [%p]
    // from the same predecessor. Each copy reads the other's destination
    // slot, so no emit order works without a temp register; isel must panic
    // loudly instead of silently miscompiling.
    let m = parse(
        "global x i8\nglobal y i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    br merge\n  block merge:\n    %p = phi i8 %q entry\n    %q = phi i8 %p entry\n    ret void\n",
    );
    // alloc: root frame at 0x25; %a=0x25, %p=0x26, %q=0x27.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("main::a", 0x25),
        ("main::p", 0x26),
        ("main::q", 0x27),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn icmp_eq_i8_materializes_i1() {
    let m = parse(
        "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: root frame at 0x25; %1=0x25, %c=0x26.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::c", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %1=0x25, %c=0x26, scratch=0x70 (fixed common RAM). epic-cc#214:
    // %1's own reload for the XOR is redundant (still in W right after
    // Inst::Load's own store to 0x25) and gets elided, so "load a" is no
    // longer its own instruction; the XOR itself is still the real check.
    assert!(asm.contains("XORLW 0x01"), "xor with const b:\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "store xor to scratch:\n{asm}");
    assert!(asm.contains("MOVLW 0x00"), "materialize 0:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 2"), "Z test:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "store d:\n{asm}");
}

#[test]
fn icmp_eq_i16_uses_scratch_accumulation() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp eq i16 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: globals end at 0x25 -> root frame at 0x28; %a=0x28, %b=0x2A,
    // %c=0x2C.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x28),
        ("main::b", 0x2A),
        ("main::c", 0x2C),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x28/29, %b=0x2A/2B, %c=0x2C, scratch=0x70 (fixed common RAM).
    assert!(asm.contains("MOVF 0x28, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("XORWF 0x2A, W"), "xor b_lo:\n{asm}");
    assert!(
        asm.contains("MOVWF 0x70"),
        "store lo xor to scratch:\n{asm}"
    );
    assert!(asm.contains("MOVF 0x29, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("XORWF 0x2B, W"), "xor b_hi:\n{asm}");
    assert!(asm.contains("IORWF 0x70, W"), "or hi into scratch:\n{asm}");
    assert!(
        asm.contains("MOVWF 0x70"),
        "store accumulated scratch:\n{asm}"
    );
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x2C"), "store d:\n{asm}");
}

#[test]
fn brcond_and_select_emit_skip_lines() {
    let m = parse(
        "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    %s = select i1 %c i8 10 i8 20\n    br i1 %c then end\n  block then:\n    store i8 %s @out\n    br end\n  block end:\n    ret void\n",
    );
    // alloc: root frame at 0x25; %1=0x25, %c=0x26, %s=0x27.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::c", 0x26),
        ("main::s", 0x27),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %1=0x25, %c=0x26, %s=0x27, scratch=0x22 (end_of_globals: 0x20+1, 0x21+1 -> 0x22).
    // brcond: cond==0 -> main_Lend (f), cond!=0 -> main_Lthen (t).
    assert!(asm.contains("MOVF 0x26, W"), "brcond reads cond:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 2"), "brcond Z test:\n{asm}");
    assert!(asm.contains("GOTO main_Lend"), "brcond f:\n{asm}");
    assert!(asm.contains("GOTO main_Lthen"), "brcond t:\n{asm}");
    // select: test cond, jump to else, copy a=10 then b=20.
    assert!(asm.contains("GOTO tmp0"), "select else jump:\n{asm}");
    assert!(asm.contains("MOVLW 0x0A"), "select copy a:\n{asm}");
    assert!(asm.contains("MOVWF 0x27"), "select dst:\n{asm}");
    assert!(asm.contains("GOTO tmp1"), "select end jump:\n{asm}");
    assert!(asm.contains("MOVLW 0x14"), "select copy b:\n{asm}");
}

#[test]
fn select_labels_are_unique_across_functions() {
    // Two functions, each containing a select. Fresh labels are file-scoped
    // in the single .asm output, so the counter must span the whole module:
    // the second function's labels must differ from the first's.
    let m = parse(
        "global a i8\nglobal b i8\nglobal o1 i8\nglobal o2 i8\n\
         fn main(void) ()\n  block entry:\n\
           %1 = load i8 @a\n    %c1 = icmp eq i8 %1, 0\n\
           %s1 = select i1 %c1 i8 1 i8 2\n    store i8 %s1 @o1\n    ret void\n\
         fn f2(void) ()\n  block entry:\n\
           %2 = load i8 @b\n    %c2 = icmp eq i8 %2, 0\n\
           %s2 = select i1 %c2 i8 3 i8 4\n    store i8 %s2 @o2\n    ret void\n",
    );
    // Both functions are roots, so alloc overlays their frames at the same
    // base (0x27 after globals ending at 0x24).
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("o1", 0x22),
        ("o2", 0x23),
        ("main::1", 0x27),
        ("main::c1", 0x28),
        ("main::s1", 0x29),
        ("f2::2", 0x27),
        ("f2::c2", 0x28),
        ("f2::s2", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Collect every emitted label *definition* (e.g. "tmp0:", not "GOTO tmp0").
    let defs: Vec<&str> = asm
        .lines()
        .filter(|l| l.trim_start().starts_with("tmp") && l.ends_with(':'))
        .collect();
    assert_eq!(
        defs.len(),
        4,
        "two selects -> 4 labels, got {defs:?}:\n{asm}"
    );
    let mut unique = defs.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        4,
        "fresh labels must be unique across functions, got {defs:?}:\n{asm}"
    );
}

#[test]
fn locals_use_map_addresses_around_scratch_and_retval() {
    // The icmp scratch byte sits in fixed common RAM (0x70), with the two
    // retval bytes just after (0x71/0x72). isel does not allocate slots any
    // more, so a local is used at exactly the map address alloc provides —
    // here 0x73/0x74, past the fixed scratch/retval bytes.
    let m = parse(
        "global in i8\nfn main(void) ()\n  block entry:\n\
           %a0 = load i8 @in\n    %c = icmp eq i8 %a0, 0\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x6F), ("main::a0", 0x73), ("main::c", 0x74)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Fixed common RAM: scratch 0x70, retval 0x71/0x72.
    assert!(
        asm.contains("MOVWF 0x70"),
        "icmp writes the fixed scratch 0x70:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF 0x71") && !asm.contains("MOVWF 0x72"),
        "no writes to the retval bytes:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x73"),
        "the load lands at the map address 0x73:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x74"),
        "icmp dst at the map address 0x74:\n{asm}"
    );
}

#[test]
fn call_copies_args_to_callee_params_and_reads_retval() {
    // %3 = call i16 @add(i16 %1, i16 %2): each arg byte is copied into the
    // callee's `{func}::{param}` slot (from the map), then CALL, then the
    // retval slots (2 bytes after the globals) are copied into the
    // destination. alloc: globals end at 0x26 -> bank0_start 0x29; main's
    // frame is 0x29..0x2E (6 bytes), add's frame follows at 0x2F.
    let m = parse(
        "global a i16\nglobal b i16\nglobal out i16\n\
         fn add(i16) (x, y)\n  block entry:\n\
           %r = add i16 %x, %y\n    ret i16 %r\n\
         fn main(void) ()\n  block entry:\n\
           %1 = load i16 @a\n    %2 = load i16 @b\n\
           %3 = call i16 @add(i16 %1, i16 %2)\n    store i16 %3 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x22),
        ("out", 0x24),
        ("main::1", 0x29),
        ("main::2", 0x2B),
        ("main::3", 0x2D),
        ("add::x", 0x2F),
        ("add::y", 0x31),
        ("add::r", 0x33),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // end_of_globals = max(0x20+2, 0x22+2, 0x24+2) = 0x26: scratch 0x26,
    // retval 0x27/0x28.
    // Arg copies: %1 -> add::x, %2 -> add::y (lo then hi).
    assert!(
        asm.contains("MOVF 0x29, W\n    MOVWF 0x2F\n    MOVF 0x2A, W\n    MOVWF 0x30"),
        "copy %1 into add::x:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x2B, W\n    MOVWF 0x31\n    MOVF 0x2C, W\n    MOVWF 0x32"),
        "copy %2 into add::y:\n{asm}"
    );
    assert!(asm.contains("    CALL add"), "CALL add:\n{asm}");
    // Retval copy: fixed retval slots 0x71/0x72 -> %3 (0x2D/0x2E).
    assert!(
        asm.contains("MOVF 0x71, W\n    MOVWF 0x2D\n    MOVF 0x72, W\n    MOVWF 0x2E"),
        "copy retval into %3:\n{asm}"
    );
}

#[test]
fn ret_i16_copies_value_to_retval_and_returns() {
    // ret i16 %v: copy %v into the fixed retval slots (0x71/0x72) then
    // RETURN.
    let m = parse(
        "global x i16\nfn main(i16) ()\n  block entry:\n\
           %v = load i16 @x\n    ret i16 %v\n",
    );
    // alloc: root frame at 0x25; %v=0x25.
    let addrs = addrs(&[("x", 0x20), ("main::v", 0x25)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %v = 0x25 (hi 0x26), retval = fixed 0x71/0x72.
    assert!(
        asm.contains("MOVF 0x25, W\n    MOVWF 0x71\n    MOVF 0x26, W\n    MOVWF 0x72"),
        "retval writes:\n{asm}"
    );
    assert!(asm.contains("    RETURN"), "RETURN:\n{asm}");
}

#[test]
fn call_arg_copies_target_callee_param_slots_from_map() {
    // The map decides where every value lives. alloc gives main's frame
    // (0x29..0x2E) and then add's frame (0x2F..0x34) — the callee's params
    // and the caller's live values get distinct addresses, so the arg-copy
    // never clobbers the caller's operands before CALL.
    let m = parse(
        "global a i16\nglobal b i16\nglobal out i16\n\
         fn add(i16) (x, y)\n  block entry:\n\
           %r = add i16 %x, %y\n    ret i16 %r\n\
         fn main(void) ()\n  block entry:\n\
           %1 = load i16 @a\n    %2 = load i16 @b\n\
           %3 = call i16 @add(i16 %1, i16 %2)\n    store i16 %3 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x22),
        ("out", 0x24),
        ("main::1", 0x29),
        ("main::2", 0x2B),
        ("main::3", 0x2D),
        ("add::x", 0x2F),
        ("add::y", 0x31),
        ("add::r", 0x33),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The caller's loads must land in main's frame slots, not the callee's
    // param slots.
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x29"),
        "main %1 lo must live at its own slot 0x29:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x22, W\n    MOVWF 0x2B"),
        "main %2 lo must live at its own slot 0x2B:\n{asm}"
    );
    // The arg-copies must read the caller's slots and write the callee's
    // distinct param slots.
    assert!(
        asm.contains("MOVF 0x29, W\n    MOVWF 0x2F"),
        "copy %1 -> add::x (distinct addresses):\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x2B, W\n    MOVWF 0x31"),
        "copy %2 -> add::y (distinct addresses):\n{asm}"
    );
}

/// Build a module whose RAM array `@ram` is 8 bytes and whose const global
/// `@table` carries the flash bytes [10, 20, 30, 40]. The IR text format
/// records `global`/`const` scalars only (sizes/bytes come from the C
/// frontend), so the globals are patched in directly, exactly as irparse
/// fills them for the e2e path.
fn pointer_module(ir_text: &str) -> ir::Module {
    let mut m = parse(ir_text);
    m.globals = vec![
        ir::Global {
            name: "in".into(),
            ty: ir::Ty::I8,
            is_const: false,
            size: 1,
            bytes: vec![0],
            addr: None,
            refs: Vec::new(),
        },
        ir::Global {
            name: "ram".into(),
            ty: ir::Ty::I8,
            is_const: false,
            size: 8,
            bytes: vec![0; 8],
            addr: None,
            refs: Vec::new(),
        },
        ir::Global {
            name: "table".into(),
            ty: ir::Ty::I8,
            is_const: true,
            size: 4,
            bytes: vec![10, 20, 30, 40],
            addr: None,
            refs: Vec::new(),
        },
    ];
    m
}

#[test]
fn gep_ram_indirect_and_const_retlw() {
    // Phase-3 pointers/const: `gep` defines a *virtual* pointer, lowered at
    // each use. A pointer into a RAM array loads/stores via FSR/INDF
    // (base_lo + offset); a pointer into a const global loads via
    // `CALL __read_table` — a RETLW table in flash. `gep` itself emits
    // nothing, so `%p`/`%t` need no slots.
    let m = pointer_module(
        "global in i8\nglobal ram i8\nconst table i8\n\
         fn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @ram +0 +1*%i\n    %t = gep @table +0 +1*%i\n\
           %v = load i8 %t\n    %w = load i8 %p\n    store i8 %v %p\n    ret void\n",
    );
    // alloc: in=0x20, ram (8 bytes) 0x21..0x28 -> end_of_globals 0x29;
    // locals in IR order: %i=0x29, %v=0x2A, %w=0x2B. `table` is const
    // (flash) — no RAM address, absent from the map.
    let addrs = addrs(&[
        ("in", 0x20),
        ("ram", 0x21),
        ("main::i", 0x29),
        ("main::v", 0x2A),
        ("main::w", 0x2B),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // RAM indirect load (%w = load i8 %p): IRP cleared first (bank-0 base
    // 0x21 — a prior bank-2/3 access would leave IRP=1), then
    // W = %i; W += 0x21 (base_lo); FSR = W; W = INDF; %w = W.
    assert!(
        asm.contains(
            "BCF STATUS, 7\n    MOVF 0x29, W\n    ADDLW 0x21\n    MOVWF FSR\n    MOVF INDF, W"
        ),
        "IRP cleared + FSR = base_lo + i for @ram:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x2B"), "RAM load dst %w:\n{asm}");
    // RAM indirect store (store i8 %v %p): same FSR setup, then W = %v;
    // INDF = W.
    assert!(asm.contains("MOVF 0x2A, W"), "store value %v:\n{asm}");
    assert!(asm.contains("MOVWF INDF"), "store through FSR:\n{asm}");
    // Const load (%v = load i8 %t): W = %i (index); CALL __read_table;
    // W -> %v.
    assert!(
        asm.contains("CALL __read_table"),
        "const load calls the table reader:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x2A"), "const load dst %v:\n{asm}");
    // The RETLW table itself, after the functions.
    assert!(asm.contains("__read_table:"), "table reader label:\n{asm}");
    // M10: every reader sets PCLATH to the table's 256-byte window before
    // the computed PCL jump (fixes the latent window bug — a table past
    // 0x100 needs PCLATH != 0 to land the jump).
    assert!(
        asm.contains("MOVLW HIGH(table)"),
        "reader must set PCLATH:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF PCLATH"),
        "reader must write PCLATH:\n{asm}"
    );
    assert!(
        asm.find("MOVLW HIGH(table)").unwrap() < asm.find("ADDLW LOW(table)").unwrap(),
        "PCLATH must be set before the computed jump:\n{asm}"
    );
    assert!(
        asm.contains("ADDLW LOW(table)"),
        "index += table base:\n{asm}"
    );
    assert!(asm.contains("MOVWF PCL"), "jump into the table:\n{asm}");
    assert!(asm.contains("table:"), "table label:\n{asm}");
    assert!(asm.contains("RETLW 0x0A"), "byte 0 (10):\n{asm}");
    assert!(asm.contains("RETLW 0x14"), "byte 1 (20):\n{asm}");
    assert!(asm.contains("RETLW 0x1E"), "byte 2 (30):\n{asm}");
    assert!(asm.contains("RETLW 0x28"), "byte 3 (40):\n{asm}");
    // The table follows main's RETURN (functions first, then tables, then
    // the __start stub).
    assert!(
        asm.find("__read_table:").unwrap() > asm.find("RETURN").unwrap(),
        "table must follow the functions:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "store to const")]
fn panics_on_store_to_const_gep() {
    // Const globals live in flash (RETLW tables); a store through a pointer
    // into one is a write to ROM and must fail loudly, never silently emit
    // a FSR/INDF store to a nonexistent RAM address.
    let m = pointer_module(
        "global in i8\nconst table i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %t = gep @table +0 +1*%i\n    store i8 %i %t\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("main::i", 0x29)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

// ---- Task 3: pointer machinery (bases, chains, FSR sums, indirect, memcpy) ----

#[test]
fn gep_const_offset_loads_direct_no_fsr() {
    // %p = gep @g +2: a constant byte offset into a RAM global is a plain
    // file-register read — no FSR setup for a statically-known address.
    let m = parse(
        "global g i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %p = gep @g +2\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    // g=0x20 (a 3+ byte array), out=0x24; locals: %v=0x29 (%p is virtual).
    let addrs = addrs(&[("g", 0x20), ("out", 0x24), ("main::v", 0x29)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x22, W"),
        "direct byte-offset load at g+2:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF FSR"),
        "no FSR setup for a constant offset:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x24"), "store to @out:\n{asm}");
}

#[test]
fn gep_single_term_uses_fsr_fast_path() {
    // %p = gep @a +1 +1*%i: one scale-1 term keeps the M5 fast shape —
    // MOVF %i,W; ADDLW <a_lo + k>; MOVWF FSR — with the constant k folded
    // into the ADDLW literal. The IRP set (BCF STATUS, 7 for the bank-0
    // base 0x21) comes first, on every FSR setup.
    let m = parse(
        "global in i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @a +1 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    // in=0x20, a=0x21 (array), out=0x25; locals: %i=0x29, %v=0x2A.
    let addrs = addrs(&[
        ("in", 0x20),
        ("a", 0x21),
        ("out", 0x25),
        ("main::i", 0x29),
        ("main::v", 0x2A),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVF 0x29, W\n    ADDLW 0x22\n    MOVWF FSR\n    MOVF INDF, W\n    MOVWF 0x2A"),
        "fast path: IRP cleared + FSR = a_lo + k + i (0x21 + 1):\n{asm}"
    );
}

#[test]
fn gep_scaled_sum_accumulates_in_scratch() {
    // %p = gep @a +1 +2*%i: a ×2-scaled term cannot fold into the fast
    // path's ADDLW — the sum accumulates in the fixed scratch byte, then
    // FSR = base + k + scratch. A two-register sum (+1*%i +1*%j) uses the
    // same accumulation in term order.
    let m = parse(
        "global in i8\nglobal jn i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %j = load i8 @jn\n    %p = gep @a +1 +2*%i\n    %v = load i8 %p\n    store i8 %v @out\n    %q = gep @a +1 +1*%i +1*%j\n    %w = load i8 %q\n    store i8 %w @out\n    ret void\n",
    );
    // in=0x20, jn=0x21, a=0x22 (array), out=0x26; locals: %i=0x29, %j=0x2A,
    // %v=0x2B, %w=0x2C. scratch = fixed 0x70.
    let addrs = addrs(&[
        ("in", 0x20),
        ("jn", 0x21),
        ("a", 0x22),
        ("out", 0x26),
        ("main::i", 0x29),
        ("main::j", 0x2A),
        ("main::v", 0x2B),
        ("main::w", 0x2C),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // scale-2 term: scratch = 2×%i — %i is reloaded into W before each
    // ADDWF (ADDWF f,W computes W = f + W), then FSR = scratch + a_lo + k.
    // The IRP set (BCF STATUS, 7) precedes the whole accumulation.
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVLW 0x00\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x23\n    MOVWF FSR"),
        "scaled term accumulates in scratch after the IRP clear:\n{asm}"
    );
    // two distinct terms accumulate in order (i then j), same FSR finish.
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVLW 0x00\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x23\n    MOVWF FSR"),
        "two-term sum accumulates both terms after the IRP clear:\n{asm}"
    );
}

#[test]
fn gep_scaled_multi_term_reloads_w_per_repetition() {
    // %p = gep @a +1 +1*%i +2*%j: a scaled term in non-first position.
    // PIC14 ADDWF f,W computes W = f + W, so W no longer holds the term
    // value after the first ADDWF — the term must be reloaded (MOVF %j,W)
    // before *each* repetition, or scale-2 computes 2*scratch + j instead
    // of scratch + 2*j (silent wrong-address miscompile).
    let m = parse(
        "global in i8\nglobal jn i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %j = load i8 @jn\n    %p = gep @a +1 +1*%i +2*%j\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    // in=0x20, jn=0x21, out=0x22, a=0x40 (array); locals: %i=0x29, %j=0x2A,
    // %v=0x2B. scratch = fixed 0x70.
    let map = [
        ("in", 0x20u16),
        ("jn", 0x21),
        ("out", 0x22),
        ("a", 0x40),
        ("main::i", 0x29),
        ("main::j", 0x2A),
        ("main::v", 0x2B),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // MOVF %j,W must appear once per scale-2 repetition — twice in total.
    assert_eq!(
        asm.matches("MOVF 0x2A, W").count(),
        2,
        "scale-2 term must reload %j into W before each ADDWF:\n{asm}"
    );
    // Full sequence: IRP cleared (bank-0 base 0x40), then
    // scratch = i + 2*j = i + j + j, then FSR = a_lo + 1 + scratch.
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVLW 0x00\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x41\n    MOVWF FSR"),
        "scaled multi-term sum accumulates i + 2*j after the IRP clear:\n{asm}"
    );
    // End-to-end: i=2, j=2 -> a[1+2+4] = a[7] = 0x17. The buggy sequence
    // (W not reloaded) computes 2*i + 2*j -> a[9] = 0x19 instead.
    let ir = "global in i8\nglobal jn i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %j = load i8 @jn\n    %p = gep @a +1 +1*%i +2*%j\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n";
    let seed = |iv: u8, jv: u8| {
        [
            (0x20u16, iv),
            (0x21, jv),
            (0x40, 0x10),
            (0x41, 0x11),
            (0x42, 0x12),
            (0x43, 0x13),
            (0x44, 0x14),
            (0x45, 0x15),
            (0x46, 0x16),
            (0x47, 0x17),
            (0x48, 0x18),
            (0x49, 0x19),
        ]
    };
    assert_eq!(
        sim_run(ir, &map, &seed(2, 2), 0x22),
        0x17,
        "a[1+2+4] with i=2, j=2"
    );
    assert_eq!(
        sim_run(ir, &map, &seed(3, 1), 0x22),
        0x16,
        "a[1+3+2] with i=3, j=1"
    );
}

#[test]
fn sret_param_store_is_indirect_via_slot_contents() {
    // An sret param slot holds the *target address*; a store through it
    // must set FSR from the slot's contents — never treat the slot itself
    // as the destination. M9: IRP is set from the stored HIGH byte first
    // (slot+1 = 0x27 here), so INDF lands in the right half of memory even
    // for a bank-2/3 target.
    let m = parse(
        "global v i8\nfn make(i8) (r=sret)\n  block entry:\n\
           %x = load i8 @v\n    %p = gep %r +0\n    store i8 %x %p\n    ret void\n",
    );
    // v=0x20; make's frame: %x=0x25, sret slot r=0x26 (2 bytes).
    let addrs = addrs(&[("v", 0x20), ("make::x", 0x25), ("make::r", 0x26)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains(
            "BTFSC 0x27, 0\n    BSF STATUS, 7\n    BTFSS 0x27, 0\n    BCF STATUS, 7\n    \
             MOVF 0x26, W\n    ADDLW 0x00\n    MOVWF FSR\n    MOVF 0x25, W\n    MOVWF INDF"
        ),
        "IRP from the stored hi byte, then FSR from the slot contents [r_lo] + k:\n{asm}"
    );
}

#[test]
fn memcpy_emits_byte_pairs() {
    // memcpy @g1 @g2 4: four MOVF src+i,W / MOVWF dst+i byte copies — no
    // loop and no FSR for direct global addresses.
    let m = parse(
        "global g1 i8\nglobal g2 i8\nfn main(void) ()\n  block entry:\n    memcpy @g1 @g2 4\n    ret void\n",
    );
    // g1=0x20 (4 bytes), g2=0x24 (4 bytes).
    let addrs = addrs(&[("g1", 0x20), ("g2", 0x24)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    for i in 0..4u16 {
        assert!(
            asm.contains(&format!(
                "MOVF 0x{:02X}, W\n    MOVWF 0x{:02X}",
                0x24 + i,
                0x20 + i
            )),
            "byte {i} copy:\n{asm}"
        );
    }
    assert!(
        !asm.contains("MOVWF FSR"),
        "direct globals need no FSR setup:\n{asm}"
    );
}

#[test]
fn alloca_based_pointer_accesses_direct_slot() {
    // %buf = alloca 4 defines a local buffer slot; %p = gep %buf +2 points
    // into it at a constant offset, so accesses are plain file-register
    // reads/writes — no FSR for a statically-known local address.
    let m = parse(
        "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %buf = alloca 4\n    %p = gep %buf +2\n    %v = load i8 @in\n    store i8 %v %p\n    %w = load i8 %p\n    store i8 %w @out\n    ret void\n",
    );
    // in=0x20, out=0x21; main's frame: %buf=0x25 (4 bytes), %v=0x29,
    // %w=0x2A; %p is virtual (no slot).
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::buf", 0x25),
        ("main::v", 0x29),
        ("main::w", 0x2A),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(asm.contains("MOVWF 0x27"), "store into buf+2:\n{asm}");
    assert!(asm.contains("MOVF 0x27, W"), "load from buf+2:\n{asm}");
    assert!(
        !asm.contains("MOVWF FSR"),
        "constant alloca offset needs no FSR setup:\n{asm}"
    );
}

#[test]
fn byval_param_base_accesses_direct_slot() {
    // A byval param slot holds the struct copy; %p = gep %0 +2 addresses a
    // field at a constant byte offset inside it — a plain file-register
    // read from the param slot.
    let m = parse(
        "global out i8\nfn f(i8) (0=byval4)\n  block entry:\n\
           %p = gep %0 +2\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    // f's frame: param slot 0=0x25 (4 bytes), %v=0x29.
    let addrs = addrs(&[("out", 0x21), ("f::0", 0x25), ("f::v", 0x29)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x27, W"),
        "byval field byte at slot+2:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF FSR"),
        "direct byval slot needs no FSR setup:\n{asm}"
    );
}

#[test]
fn fsr_setup_in_each_of_the_four_banks() {
    // M9: FSR reaches all four banks via the IRP bit. A dynamic-index base
    // in bank 0 (0x20) or bank 1 (0xA0) clears IRP (BCF STATUS, 7); bank 2
    // (0x120) or bank 3 (0x1A0) sets it (BSF STATUS, 7). The ADDLW literal
    // is always (base + k) & 0xFF: 0x120 -> 0x20, 0x1A0 -> 0xA0.
    for (base, irp_line, lit) in [
        (0x20u16, "BCF STATUS, 7", "ADDLW 0x20"),
        (0xA0, "BCF STATUS, 7", "ADDLW 0xA0"),
        (0x120, "BSF STATUS, 7", "ADDLW 0x20"),
        (0x1A0, "BSF STATUS, 7", "ADDLW 0xA0"),
    ] {
        let m = parse(
            "global in i8\nglobal g i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
               %i = load i8 @in\n    %p = gep @g +0 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
        );
        // in=0x24, g=base (1 byte), out=0x28; locals %i=0x29, %v=0x2A.
        let addrs = addrs(&[
            ("in", 0x24),
            ("g", base),
            ("out", 0x28),
            ("main::i", 0x29),
            ("main::v", 0x2A),
        ]);
        let asm = select(&PIC16F877A, &m, &addrs);
        assert!(
            asm.contains(&format!(
                "{irp_line}\n    MOVF 0x29, W\n    {lit}\n    MOVWF FSR"
            )),
            "base 0x{base:03X}: {irp_line} then FSR = (base + i) & 0xFF:\n{asm}"
        );
    }
}

#[test]
#[should_panic(expected = "crosses window end 0x80")]
fn panics_on_fsr_object_crossing_the_0x80_hole() {
    // A 16-byte object at 0x78 spans 0x78..0x87: byte 0x80 is the first SFR
    // hole byte. An FSR/INDF access would silently mis-address into the
    // SFRs, so the window check fails loudly. (The object's span comes from
    // the global's size — 16 here, patched in as irparse fills it.)
    let mut m = parse(
        "global in i8\nglobal g i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @g +0 +1*%i\n    %v = load i8 %p\n    ret void\n",
    );
    m.globals[1].size = 16;
    let addrs = addrs(&[
        ("in", 0x24),
        ("g", 0x78),
        ("main::i", 0x29),
        ("main::v", 0x2A),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
#[should_panic(expected = "crosses window end 0x170")]
fn panics_on_fsr_object_crossing_the_0x170_hole() {
    // A 33-byte object at 0x150 spans 0x150..0x170 inclusive of the first
    // SFR-hole byte 0x170. (Note 0x150 + 32 = 0x170 would FIT exactly — the
    // last GPR byte is 0x16F — so 33 is the smallest crossing span.)
    let mut m = parse(
        "global in i8\nglobal g i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @g +0 +1*%i\n    %v = load i8 %p\n    ret void\n",
    );
    m.globals[1].size = 33;
    let addrs = addrs(&[
        ("in", 0x24),
        ("g", 0x150),
        ("main::i", 0x29),
        ("main::v", 0x2A),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn byval_param_slot_in_bank_2_sets_irp() {
    // A byval param slot at 0x120 (width 16) dynamically indexed: the FSR
    // setup must set IRP (BSF STATUS, 7) and use the low byte of the base
    // (ADDLW 0x20). The span comes from the param's width.
    let m = parse(
        "global in i8\nglobal out i8\nfn f(i8) (0=byval16)\n  block entry:\n\
           %i = load i8 @in\n    %p = gep %0 +0 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    // in=0x20, out=0x21; f's frame: %i=0x25, param slot 0=0x120 (16 bytes),
    // %v=0x29.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("f::i", 0x25),
        ("f::0", 0x120),
        ("f::v", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("BSF STATUS, 7\n    MOVF 0x25, W\n    ADDLW 0x20\n    MOVWF FSR"),
        "bank-2 byval slot: IRP set + FSR = (0x120 + i) & 0xFF:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "cyclic")]
fn panics_on_cyclic_gep_chain() {
    // %p = gep %q +0; %q = gep %p +0: neither chain can fold to a base —
    // isel must panic loudly instead of looping forever or miscompiling.
    let m = parse(
        "global a i8\nfn main(void) ()\n  block entry:\n\
           %p = gep %q +0\n    %q = gep %p +0\n    ret void\n",
    );
    let _ = select(&PIC16F877A, &m, &HashMap::new());
}

#[test]
fn gep_chain_s8_pattern_simulates() {
    // The s8 pattern: %p = gep @a +1; %q = gep %p +1 +1*%i — a GEP whose
    // base is another GEP. The chain folds to @a + 2 + i; the sim must
    // read exactly that byte. in=0x20, a=0x21..0x24, out=0x25; locals
    // %i=0x29, %v=0x2A.
    let ir = "global in i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %p = gep @a +1\n    %q = gep %p +1 +1*%i\n    %v = load i8 %q\n    store i8 %v @out\n    ret void\n";
    let map = [
        ("in", 0x20u16),
        ("a", 0x21),
        ("out", 0x25),
        ("main::i", 0x29),
        ("main::v", 0x2A),
    ];
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The chain folds k = 1 + 1 = 2 into the fast path's ADDLW literal;
    // the IRP clear (bank-0 base) precedes the FSR setup.
    assert!(
        asm.contains(
            "BCF STATUS, 7\n    MOVF 0x29, W\n    ADDLW 0x23\n    MOVWF FSR\n    MOVF INDF, W"
        ),
        "chained gep folds to @a + 2 + i (a_lo 0x21 + 2 = 0x23):\n{asm}"
    );
    // in = 1 -> a[2+1] = a[3] = 0x44; in = 0 -> a[2] = 0x33.
    let seed = [
        (0x20u16, 1u8),
        (0x21, 0x11),
        (0x22, 0x22),
        (0x23, 0x33),
        (0x24, 0x44),
    ];
    assert_eq!(sim_run(ir, &map, &seed, 0x25), 0x44, "a[2+1] with i=1");
    assert_eq!(
        sim_run(
            ir,
            &map,
            &[
                (0x20, 0),
                (0x21, 0x11),
                (0x22, 0x22),
                (0x23, 0x33),
                (0x24, 0x44)
            ],
            0x25,
        ),
        0x33,
        "a[2+0] with i=0"
    );
}

// ---------------------------------------------------------------------------
// Milestone 9, Task 1: multi-bank FSR via IRP — dynamic-indexed arrays in
// banks 1-3 write + read back in the simulator, plus an interleaved
// bank-0/bank-2 sequence proving IRP is re-set on EVERY FSR setup.
// ---------------------------------------------------------------------------

#[test]
fn banked_arrays_write_and_read_back_in_sim() {
    // M9 acceptance: `%p = gep @aN +0 +1*%i` with @aN in bank 1 (0xA0),
    // bank 2 (0x120) and bank 3 (0x1A0). Each store's FSR setup sets IRP
    // (BSF for banks 2/3, BCF for bank 1) so INDF lands in the right bank;
    // each load's FSR setup re-sets it. out = a1[3] + a2[3] + a3[3] =
    // 0x11 + 0x22 + 0x33 = 0x66. (IR consts are decimal: 17/34/51.)
    let ir = "global in i8\nglobal a1 i8\nglobal a2 i8\nglobal a3 i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n\
           %p1 = gep @a1 +0 +1*%i\n\
           %p2 = gep @a2 +0 +1*%i\n\
           %p3 = gep @a3 +0 +1*%i\n\
           store i8 17 %p1\n\
           store i8 34 %p2\n\
           store i8 51 %p3\n\
           %r1 = load i8 %p1\n\
           %r2 = load i8 %p2\n\
           %r3 = load i8 %p3\n\
           %s1 = add i8 %r1, %r2\n\
           %s2 = add i8 %s1, %r3\n\
           store i8 %s2 @out\n\
           ret void\n";
    // in=0x20, a1=0xA0 (bank 1), a2=0x120 (bank 2), a3=0x1A0 (bank 3),
    // out=0x24; locals 0x25+ (all bank 0 — the raw emitted asm needs no
    // banking since only FSR/INDF touches the banked arrays).
    let map = [
        ("in", 0x20u16),
        ("a1", 0xA0),
        ("a2", 0x120),
        ("a3", 0x1A0),
        ("out", 0x24),
        ("main::i", 0x25),
        ("main::r1", 0x26),
        ("main::r2", 0x27),
        ("main::r3", 0x28),
        ("main::s1", 0x29),
        ("main::s2", 0x2A),
    ];
    assert_eq!(
        sim_run(ir, &map, &[(0x20, 3)], 0x24),
        0x66,
        "writes+reads across banks 1/2/3: 0x11 + 0x22 + 0x33"
    );
}

#[test]
fn interleaved_bank0_bank2_accesses_reset_irp() {
    // The load-bearing IRP hazard: a bank-2 FSR access leaves IRP=1, so a
    // following bank-0 access MUST re-clear it (BCF STATUS, 7) or INDF
    // silently hits 0x120 again. Sequence: b0[2]=0xAA (IRP 0), b2[2]=0xBB
    // (IRP 1), b0[2]=0xCC (IRP re-cleared), then read b2[2] (IRP re-set)
    // and b0[2] (IRP re-cleared): out = 0xBB + 0xCC = 0x87. With the re-set
    // bug, the 0xCC write lands at 0x122 (clobbering b2[2]) and both reads
    // hit 0x122: out = 0xCC + 0xCC = 0x98. (IR consts are decimal:
    // 0xAA=170, 0xBB=187, 0xCC=204; expected out 0x87 = 135.)
    let ir = "global in i8\nglobal b0 i8\nglobal b2 i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n\
           %q0 = gep @b0 +0 +1*%i\n\
           %q2 = gep @b2 +0 +1*%i\n\
           store i8 170 %q0\n\
           store i8 187 %q2\n\
           store i8 204 %q0\n\
           %v = load i8 %q2\n\
           %w = load i8 %q0\n\
           %s = add i8 %v, %w\n\
           store i8 %s @out\n\
           ret void\n";
    // b0=0x20 (8 bytes), b2=0x120 (8 bytes); in=0x28, out=0x29; locals
    // 0x2A+.
    let map = [
        ("in", 0x28u16),
        ("b0", 0x20),
        ("b2", 0x120),
        ("out", 0x29),
        ("main::i", 0x2A),
        ("main::v", 0x2B),
        ("main::w", 0x2C),
        ("main::s", 0x2D),
    ];
    assert_eq!(
        sim_run(ir, &map, &[(0x28, 2)], 0x29),
        0x87,
        "interleaved bank-0/bank-2: IRP re-set per access (0xBB + 0xCC)"
    );
}

#[test]
fn parse_map_accepts_const_lines() {
    // alloc's map text lists const globals as `const <name>` (no address —
    // their bytes live in flash). parse_map must accept the line without
    // recording an address, and keep parsing the lines after it.
    let addrs = isel::parse_map("global in 0x20\nconst table\nlocal main i 0x29\n");
    assert_eq!(addrs.get("in"), Some(&0x20u16));
    assert_eq!(addrs.get("main::i"), Some(&0x29u16));
    assert!(
        !addrs.contains_key("table"),
        "const globals have no RAM address"
    );
}

#[test]
fn sub_i8_reg_reg_emits_subwf() {
    // d = a - b: MOVF b,W (W=b); SUBWF a,W (W = a - W = a - b); MOVWF d.
    let m = parse(
        "global x i8\nglobal y i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r = sub i8 %a, %b\n    store i8 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x23 -> root frame at 0x26; %a=0x26, %b=0x27, %r=0x28.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("out", 0x22),
        ("main::a", 0x26),
        ("main::b", 0x27),
        ("main::r", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x26, %b=0x27, %r=0x28.
    assert!(asm.contains("MOVF 0x27, W"), "load b:\n{asm}");
    assert!(asm.contains("SUBWF 0x26, W"), "a - b:\n{asm}");
    assert!(asm.contains("MOVWF 0x28"), "store d:\n{asm}");
}

#[test]
fn sub_i8_reg_const_emits_subwf_in_correct_direction() {
    // d = a - k: MOVLW k (W=k); SUBWF a,W (W = a - W = a - k); MOVWF d.
    // SUBWF f,W always computes f - W, so the const goes in W via MOVLW and
    // the register is the file operand — never SUBLW (which would be k - a).
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %r = sub i8 %a, 5\n    store i8 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x22 -> root frame at 0x25; %a=0x25, %r=0x26.
    let addrs = addrs(&[
        ("x", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::r", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x25, %r=0x26.
    assert!(asm.contains("MOVLW 0x05"), "load k into W:\n{asm}");
    assert!(asm.contains("SUBWF 0x25, W"), "a - k via SUBWF a,W:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "store d:\n{asm}");
    // Direction guard: SUBLW would compute k - a (wrong direction).
    assert!(
        !asm.contains("SUBLW"),
        "reg-const sub must not use SUBLW:\n{asm}"
    );
}

#[test]
fn negative_i8_const_is_masked_to_byte() {
    // clang prints an i8 constant >= 128 as a negative i8 (`sub i8 %a, -42`
    // for `a - 214u`); the literal lowering must mask it to the byte
    // (0xD6 = 214 = -42 mod 256) rather than sign-extend to a 16-bit value
    // or panic (found by the fuzz corpus; parity with the other
    // corpus-found constant-masking fixes).
    let m = parse(
        "global x i8\nglobal o1 i8\nglobal o2 i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %s = sub i8 %a, -42\n    store i8 %s @o1\n    %t = add i8 %a, -42\n    store i8 %t @o2\n    ret void\n",
    );
    // alloc: x=0x20, o1=0x21, o2=0x22 -> end 0x23 -> root frame at 0x26;
    // %a=0x26, %s=0x27, %t=0x28.
    let addrs = addrs(&[
        ("x", 0x20),
        ("o1", 0x21),
        ("o2", 0x22),
        ("main::a", 0x26),
        ("main::s", 0x27),
        ("main::t", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // sub: MOVLW 0xD6 (W = -42 & 0xFF); SUBWF a,W (W = a - 214); MOVWF s.
    assert!(
        asm.contains("MOVLW 0xD6\n    SUBWF 0x26, W"),
        "sub masks -42 to 0xD6:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x27"), "sub dst:\n{asm}");
    // add: MOVF a,W; ADDLW 0xD6; MOVWF t.
    assert!(
        asm.contains("MOVF 0x26, W\n    ADDLW 0xD6"),
        "add masks -42 to 0xD6:\n{asm}"
    );
    assert!(asm.contains("MOVWF 0x28"), "add dst:\n{asm}");
    // No sign-extended high literal leaks in: an i8 -42 must not emit a
    // 0xFF fill (that would be a 16-bit constant, not a masked i8).
    assert!(
        !asm.contains("MOVLW 0xFF"),
        "no sign-extension leak:\n{asm}"
    );

    // Borrow semantics: SUBWF f,W sets C=0 on borrow (f < W), and the
    // single-byte result wraps mod 256. a=10, k=214 (-42) -> 10 - 214 =
    // -204 mod 256 = 0x34. WORDS: MOVLW 0xD6(0x30D6) SUBWF a,W(0x0220)
    // MOVWF d(0x00A4). RAM: a=0x20, d=0x24.
    use pic14_sim::Pic14;
    let mut p = Pic14::new(vec![0x30D6, 0x0220, 0x00A4]);
    p.ram_mut()[0x20] = 10;
    p.run(1000);
    assert_eq!(
        p.ram()[0x24],
        0x34,
        "negative-const sub must borrow/wrap correctly"
    );
}

#[test]
fn sub_i16_reg_reg_emits_borrow_chain() {
    // d = a - b (i16): lo byte SUBWF, then hi byte with a borrow folded in
    // via BTFSS STATUS,0 / ADDLW 1 before the hi SUBWF.
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %r = sub i16 %a, %b\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2B, %r=0x2D.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::r", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x29 (hi 0x2A), %b=0x2B (hi 0x2C), %r=0x2D (hi 0x2E).
    assert!(asm.contains("MOVF 0x2B, W"), "load b_lo:\n{asm}");
    assert!(asm.contains("SUBWF 0x29, W"), "a_lo - b_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x2D"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVF 0x2C, W"), "load b_hi:\n{asm}");
    assert!(asm.contains("BTFSS STATUS, 0"), "borrow test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "borrow-in add:\n{asm}");
    assert!(asm.contains("SUBWF 0x2A, W"), "a_hi - b_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x2E"), "store d_hi:\n{asm}");
}

#[test]
fn sub_i16_reg_const_emits_borrow_chain() {
    // 515 = 0x0203 -> lo 0x03, hi 0x02 (hi differs from the borrow ADDLW
    // 0x01, so the k_hi MOVLW line is distinguishable).
    let m = parse(
        "global x i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %r = sub i16 %a, 515\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x24 -> root frame at 0x27; %a=0x27, %r=0x29.
    let addrs = addrs(&[
        ("x", 0x20),
        ("out", 0x22),
        ("main::a", 0x27),
        ("main::r", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x27 (hi 0x28), %r=0x29 (hi 0x2A).
    assert!(asm.contains("MOVLW 0x03"), "load k_lo:\n{asm}");
    assert!(asm.contains("SUBWF 0x27, W"), "a_lo - k_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x29"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVLW 0x02"), "load k_hi:\n{asm}");
    assert!(asm.contains("BTFSS STATUS, 0"), "borrow test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "borrow-in add:\n{asm}");
    assert!(asm.contains("SUBWF 0x28, W"), "a_hi - k_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store d_hi:\n{asm}");
}

#[test]
fn sub_const_lhs_emits_sublw_chain() {
    // sub is NOT commutative: d = k - a cannot reuse the reg-const lowering
    // (which computes a - k) and must never read a const as a file register.
    // The const-LHS path lowers byte 0 as `MOVF a,W; SUBLW k; MOVWF d`
    // (SUBLW computes k - W), then each higher byte preloads k_i into the
    // destination and folds the borrow with the wrap-correct INCFSZ idiom
    // (issue #1): a_i is stashed in the scratch, `INCFSZ scratch, W` skips
    // the in-place `SUBWF d_i, F` when a_i + borrow wraps to 0x100, leaving
    // d_i = k_i — the correct mod-256 result — with C = borrow-in, the
    // true borrow-out.
    //
    // i8: d = 5 - a.
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %r = sub i8 5, %a\n    store i8 %r @out\n    ret void\n",
    );
    let addrs8 = addrs(&[
        ("x", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::r", 0x26),
    ]);
    let asm8 = select(&PIC16F877A, &m, &addrs8);
    // %a=0x25, %r=0x26.
    assert!(asm8.contains("MOVF 0x25, W"), "load a:\n{asm8}");
    assert!(asm8.contains("SUBLW 0x05"), "k - a via SUBLW k:\n{asm8}");
    assert!(asm8.contains("MOVWF 0x26"), "store d:\n{asm8}");
    assert!(
        !asm8.contains("SUBWF"),
        "const-LHS sub must not use SUBWF (a - k is the wrong direction):\n{asm8}"
    );

    // i16: d = 0x1234 - a. 0x1234 -> lo 0x34, hi 0x12.
    let m = parse(
        "global x i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %r = sub i16 4660, %a\n    store i16 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x24 -> root frame at 0x27; %a=0x27, %r=0x29.
    let addrs16 = addrs(&[
        ("x", 0x20),
        ("out", 0x22),
        ("main::a", 0x27),
        ("main::r", 0x29),
    ]);
    let asm16 = select(&PIC16F877A, &m, &addrs16);
    // %a=0x27 (hi 0x28), %r=0x29 (hi 0x2A), scratch=0x70.
    assert!(asm16.contains("MOVF 0x27, W"), "load a_lo:\n{asm16}");
    assert!(asm16.contains("SUBLW 0x34"), "k_lo - a_lo:\n{asm16}");
    assert!(asm16.contains("MOVWF 0x29"), "store d_lo:\n{asm16}");
    assert!(asm16.contains("MOVF 0x28, W"), "load a_hi:\n{asm16}");
    assert!(
        asm16.contains("MOVWF 0x70"),
        "a_hi stashed in the scratch:\n{asm16}"
    );
    assert!(asm16.contains("MOVLW 0x12"), "preload k_hi:\n{asm16}");
    assert!(asm16.contains("MOVWF 0x2A"), "preload d_hi:\n{asm16}");
    assert!(asm16.contains("BTFSS STATUS, 0"), "borrow test:\n{asm16}");
    assert!(
        asm16.contains("INCFSZ 0x70, W"),
        "wrap-correct borrow fold:\n{asm16}"
    );
    assert!(
        asm16.contains("SUBWF 0x2A, F"),
        "k_hi - a_hi in place:\n{asm16}"
    );
    assert!(!asm16.contains("SUBWF 0x27") && !asm16.contains("SUBWF 0x28"), "const-LHS sub must never subtract from the source a (a - k is the wrong direction):\n{asm16}");

    // i32: d = 0x12345678 - a, a four-byte SUBLW borrow chain.
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 305419896, %a\n    store i32 %r @out\n    ret void\n",
    );
    let addrs32 = addrs(&[
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::r", 0x34),
    ]);
    let asm32 = select(&PIC16F877A, &m, &addrs32);
    // %a=0x30..0x33, %r=0x34..0x37, scratch=0x70; 0x12345678 -> bytes
    // 0x78, 0x56, 0x34, 0x12. Each higher byte: a_i -> scratch, k_i
    // preloaded into d_i, then the wrap-correct INCFSZ fold + in-place
    // SUBWF d_i, F.
    assert!(asm32.contains("MOVF 0x30, W"), "load a_b0:\n{asm32}");
    assert!(asm32.contains("SUBLW 0x78"), "k_b0 - a_b0:\n{asm32}");
    assert!(asm32.contains("MOVWF 0x34"), "store d_b0:\n{asm32}");
    assert!(asm32.contains("MOVF 0x31, W"), "load a_b1:\n{asm32}");
    assert!(
        asm32.contains("MOVWF 0x70"),
        "a_b1 stashed in the scratch:\n{asm32}"
    );
    assert!(asm32.contains("MOVLW 0x56"), "preload k_b1:\n{asm32}");
    assert!(asm32.contains("MOVWF 0x35"), "preload d_b1:\n{asm32}");
    assert!(asm32.contains("BTFSS STATUS, 0"), "borrow test:\n{asm32}");
    assert!(
        asm32.contains("INCFSZ 0x70, W"),
        "wrap-correct borrow fold:\n{asm32}"
    );
    assert!(
        asm32.contains("SUBWF 0x35, F"),
        "k_b1 - a_b1 in place:\n{asm32}"
    );
    assert!(asm32.contains("MOVF 0x32, W"), "load a_b2:\n{asm32}");
    assert!(asm32.contains("MOVLW 0x34"), "preload k_b2:\n{asm32}");
    assert!(asm32.contains("MOVWF 0x36"), "preload d_b2:\n{asm32}");
    assert!(
        asm32.contains("SUBWF 0x36, F"),
        "k_b2 - a_b2 in place:\n{asm32}"
    );
    assert!(asm32.contains("MOVF 0x33, W"), "load a_b3:\n{asm32}");
    assert!(asm32.contains("MOVLW 0x12"), "preload k_b3:\n{asm32}");
    assert!(asm32.contains("MOVWF 0x37"), "preload d_b3:\n{asm32}");
    assert!(
        asm32.contains("SUBWF 0x37, F"),
        "k_b3 - a_b3 in place:\n{asm32}"
    );
    assert!(
        !asm32.contains("SUBWF 0x30")
            && !asm32.contains("SUBWF 0x31")
            && !asm32.contains("SUBWF 0x32")
            && !asm32.contains("SUBWF 0x33"),
        "const-LHS sub must never subtract from the source a:\n{asm32}"
    );
}

#[test]
fn loop_with_cross_referencing_phis_simulates() {
    // clang -O1 folds `for (i = 0; i < n; i++) acc = i;` (its SCEV proves
    // `acc` IS the induction) into two loop phis where one's incoming is
    // the other's register: %5 (acc) <- %4 (i), %4 (i) <- %6 (i+1). The
    // phi copies for the back edge must run ONLY on the back edge and read
    // the OLD i first — running them on the exit edge (or copying after i
    // is updated) clobbers the accumulator the exit block reads (found by
    // the fuzz corpus: PIC 228 vs host 230). acc = n-1 for n >= 1.
    let ir = "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = and i8 %1, 7\n    br loop\n  block loop:\n    %4 = phi i8 0 entry %6 loop\n    %5 = phi i8 0 entry %4 loop\n    %6 = add i8 %4, 1\n    %7 = icmp eq i8 %4, %2\n    br i1 %7 exit loop\n  block exit:\n    store i8 %5 @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::4", 0x27),
        ("main::5", 0x28),
        ("main::6", 0x29),
        ("main::7", 0x2A),
    ];
    for (seed_byte, expect) in [
        (0u8, 0u8), // n = 0: the loop body never runs, acc = 0
        (105, 0),   // n = 1: acc = 0 (the old i of the only body run)
        (2, 1),     // n = 2: acc = 1
        (7, 6),     // n = 7: acc = 6
    ] {
        let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, seed_byte)], 0x21, 1);
        assert_eq!(
            got[0],
            expect,
            "acc for n = {} (in0 = {seed_byte})",
            seed_byte & 7
        );
    }
}

#[test]
fn separate_latch_back_edge_cross_referencing_phis_simulates() {
    // A TWO-BLOCK loop (a latch block + the merge header) with
    // cross-referencing phis: %acc = phi [0, entry] [%i, latch] feeds off
    // the header's %i phi, so on the latch -> header back edge the %acc
    // copy must read the OLD i before the %i copy overwrites the slot. The
    // pred != merge here (unlike the folded self-loop above), so the old
    // pred == merge discriminator picked writer-first: `%i <- %i.next` then
    // `%acc <- %i` made the accumulator read the NEW induction value —
    // acc = n instead of n-1. acc = n-1 for n >= 1.
    let ir = "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = and i8 %1, 7\n    br header\n  block header:\n    %i = phi i8 0 entry %i.next latch\n    %acc = phi i8 0 entry %i latch\n    %7 = icmp eq i8 %i, %2\n    br i1 %7 exit latch\n  block latch:\n    %i.next = add i8 %i, 1\n    br header\n  block exit:\n    store i8 %acc @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::i", 0x27),
        ("main::acc", 0x28),
        ("main::7", 0x29),
        ("main::i.next", 0x2A),
    ];
    for (seed_byte, expect) in [
        (0u8, 0u8), // n = 0: the loop body never runs, acc = 0
        (105, 0),   // n = 1: acc = 0 (the old i of the only latch run)
        (2, 1),     // n = 2: acc = 1
        (7, 6),     // n = 7: acc = 6
    ] {
        let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, seed_byte)], 0x21, 1);
        assert_eq!(
            got[0],
            expect,
            "acc for n = {} (in0 = {seed_byte})",
            seed_byte & 7
        );
    }
}

#[test]
fn and_i8_uses_andwf_andlw() {
    // reg-reg: ANDWF a,W; MOVWF d (b's own reload is elided: epic-cc#217
    // wires emit_commutative through the W-tracking cache #214 established,
    // and b's value is still in W from its own immediately preceding
    // store). reg-const: MOVF a,W; ANDLW k.
    let m = parse(
        "global x i8\nglobal y i8\nglobal o1 i8\nglobal o2 i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r1 = and i8 %a, %b\n    store i8 %r1 @o1\n    %r2 = and i8 %a, 5\n    store i8 %r2 @o2\n    ret void\n",
    );
    // alloc: x=0x20,y=0x21,o1=0x22,o2=0x23 -> end 0x24 -> root 0x27;
    // %a=0x27, %b=0x28, %r1=0x29, %r2=0x2A.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o1", 0x22),
        ("o2", 0x23),
        ("main::a", 0x27),
        ("main::b", 0x28),
        ("main::r1", 0x29),
        ("main::r2", 0x2A),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // reg-reg: %a=0x27, %b=0x28, %r1=0x29. b's reload is elided: its value
    // is already in W from the immediately preceding `%b = load i8 @y`.
    assert!(
        !asm.contains("MOVF 0x28, W"),
        "b's reload should be elided:\n{asm}"
    );
    assert!(asm.contains("ANDWF 0x27, W"), "a & b:\n{asm}");
    assert!(asm.contains("MOVWF 0x29"), "store d1:\n{asm}");
    // reg-const: %a=0x27, %r2=0x2A. a's reload is not elided here: a
    // global store to @o1 sits between it and a's own last load.
    assert!(asm.contains("MOVF 0x27, W"), "reload a:\n{asm}");
    assert!(asm.contains("ANDLW 0x05"), "a & 5:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store d2:\n{asm}");
}

#[test]
fn or_i8_and_i16_use_ior() {
    let m = parse(
        "global x i8\nglobal y i8\nglobal o8 i8\nglobal p i16\nglobal q i16\nglobal o16 i16\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r = or i8 %a, %b\n    store i8 %r @o8\n    %c = load i16 @p\n    %d = load i16 @q\n    %s = or i16 %c, %d\n    store i16 %s @o16\n    ret void\n",
    );
    // alloc: x=0x20,y=0x21,o8=0x22,p(i16)=0x24,q=0x26,o16=0x28 -> end 0x2A ->
    // root 0x2D; %a=0x2D, %b=0x2E, %r=0x2F, %c=0x30, %d=0x32, %s=0x34.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o8", 0x22),
        ("p", 0x24),
        ("q", 0x26),
        ("o16", 0x28),
        ("main::a", 0x2D),
        ("main::b", 0x2E),
        ("main::r", 0x2F),
        ("main::c", 0x30),
        ("main::d", 0x32),
        ("main::s", 0x34),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // i8 reg-reg: %a=0x2D, %b=0x2E, %r=0x2F.
    assert!(asm.contains("IORWF 0x2D, W"), "i8 or:\n{asm}");
    assert!(asm.contains("MOVWF 0x2F"), "i8 or dst:\n{asm}");
    // i16 reg-reg: %c=0x30/31, %d=0x32/33, %s=0x34/35.
    assert!(asm.contains("IORWF 0x30, W"), "i16 or lo:\n{asm}");
    assert!(asm.contains("IORWF 0x31, W"), "i16 or hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x34"), "i16 or dst_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x35"), "i16 or dst_hi:\n{asm}");
}

#[test]
fn or_const_lhs_swaps_to_iorlw() {
    // Commutative or: a const LHS is swapped to the RHS so the IORLW path is
    // used, never reading a const as a file-register address.
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %r = or i8 5, %a\n    store i8 %r @out\n    ret void\n",
    );
    // alloc: globals end at 0x22 -> root frame at 0x25; %a=0x25, %r=0x26.
    let addrs = addrs(&[
        ("x", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::r", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("IORLW 0x05"),
        "const-LHS or should use the IORLW path:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x26"),
        "the result lands at its map address:\n{asm}"
    );
    assert!(
        !asm.contains("IORWF 0x05"),
        "const must not be read as a file register:\n{asm}"
    );
}

#[test]
fn xor_i8_and_i16_use_xor() {
    let m = parse(
        "global x i8\nglobal y i8\nglobal o8 i8\nglobal p i16\nglobal q i16\nglobal o16 i16\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r = xor i8 %a, %b\n    store i8 %r @o8\n    %c = load i16 @p\n    %d = load i16 @q\n    %s = xor i16 %c, %d\n    store i16 %s @o16\n    ret void\n",
    );
    // alloc: x=0x20,y=0x21,o8=0x22,p(i16)=0x24,q=0x26,o16=0x28 -> end 0x2A ->
    // root 0x2D; %a=0x2D, %b=0x2E, %r=0x2F, %c=0x30, %d=0x32, %s=0x34.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o8", 0x22),
        ("p", 0x24),
        ("q", 0x26),
        ("o16", 0x28),
        ("main::a", 0x2D),
        ("main::b", 0x2E),
        ("main::r", 0x2F),
        ("main::c", 0x30),
        ("main::d", 0x32),
        ("main::s", 0x34),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // i8 reg-reg: %a=0x2D, %b=0x2E, %r=0x2F.
    assert!(asm.contains("XORWF 0x2D, W"), "i8 xor:\n{asm}");
    assert!(asm.contains("MOVWF 0x2F"), "i8 xor dst:\n{asm}");
    // i16 reg-reg: %c=0x30/31, %d=0x32/33, %s=0x34/35.
    assert!(asm.contains("XORWF 0x30, W"), "i16 xor lo:\n{asm}");
    assert!(asm.contains("XORWF 0x31, W"), "i16 xor hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x34"), "i16 xor dst_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x35"), "i16 xor dst_hi:\n{asm}");
}

#[test]
fn sub_direction_simulates_correctly() {
    // Semantic check of the exact sequences isel emits for sub — confirming
    // the direction (d = a - b / a - k) and the i16 borrow chain, since the
    // PIC14 SUBWF/SUBLW/BTFSS semantics are the load-bearing part. Words are
    // hand-encoded to mirror isel's emitted code (SIM: SUBWF f,W = f - W;
    // BTFSS STATUS,0 skips the ADDLW when C is set, i.e. no borrow).
    // RAM: a=0x20(lo)/0x21(hi), b=0x22(lo)/0x23(hi), d=0x24(lo)/0x25(hi).
    use pic14_sim::Pic14;
    // RAM: a=0x20(lo)/0x21(hi), b=0x22(lo)/0x23(hi), d=0x24(lo)/0x25(hi).
    // RAM must be seeded before run — the sim halts once pc passes the end.

    // sub i8 reg-reg: MOVF b,W(0x0822) SUBWF a,W(0x0220) MOVWF d(0x00A4).
    // a=0x20=10, b=0x22=3 -> d = 7.
    {
        let mut p = Pic14::new(vec![0x0822, 0x0220, 0x00A4]);
        p.ram_mut()[0x20] = 10;
        p.ram_mut()[0x22] = 3;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 7, "reg-reg sub must compute a - b");
    }

    // sub i8 reg-const: MOVLW 3(0x3003) SUBWF a,W(0x0220) MOVWF d(0x00A4).
    // a=0x20=10 -> d = 10 - 3 = 7 (NOT 3 - 10 = 249).
    {
        let mut p = Pic14::new(vec![0x3003, 0x0220, 0x00A4]);
        p.ram_mut()[0x20] = 10;
        p.run(1000);
        assert_eq!(
            p.ram()[0x24],
            7,
            "reg-const sub must compute a - k, not k - a"
        );
    }

    // sub i16 reg-reg with borrow: a=0x0105, b=0x0007 -> d = 0x00FE.
    // MOVF b_lo(0x0822) SUBWF a_lo(0x0220) MOVWF d_lo(0x00A4)
    // MOVF b_hi(0x0823) BTFSS STATUS,0(0x1C03) ADDLW 1(0x3E01)
    // SUBWF a_hi(0x0221) MOVWF d_hi(0x00A5)
    {
        let mut p = Pic14::new(vec![
            0x0822, 0x0220, 0x00A4, 0x0823, 0x1C03, 0x3E01, 0x0221, 0x00A5,
        ]);
        p.ram_mut()[0x20] = 0x05; // a_lo
        p.ram_mut()[0x21] = 0x01; // a_hi
        p.ram_mut()[0x22] = 0x07; // b_lo
        p.ram_mut()[0x23] = 0x00; // b_hi
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0xFE, "i16 sub lo with borrow");
        assert_eq!(p.ram()[0x25], 0x00, "i16 sub hi with borrow");
    }

    // sub i16 reg-const with borrow: a=0x0105, k=0x0007 -> d = 0x00FE.
    // MOVLW 7(0x3007) SUBWF a_lo(0x0220) MOVWF d_lo(0x00A4)
    // MOVLW 0(0x3000) BTFSS STATUS,0(0x1C03) ADDLW 1(0x3E01)
    // SUBWF a_hi(0x0221) MOVWF d_hi(0x00A5)
    {
        let mut p = Pic14::new(vec![
            0x3007, 0x0220, 0x00A4, 0x3000, 0x1C03, 0x3E01, 0x0221, 0x00A5,
        ]);
        p.ram_mut()[0x20] = 0x05; // a_lo
        p.ram_mut()[0x21] = 0x01; // a_hi
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0xFE, "i16 const sub lo with borrow");
        assert_eq!(p.ram()[0x25], 0x00, "i16 const sub hi with borrow");
    }
}

// ---- Task 3: all icmp predicates ----

// The compare prefix for "a op b" is MOVF b,W; SUBWF a,W: SUBWF f,W computes
// f - W, so C = (a >= b) and Z = (a == b) unsigned. The predicate
// materializations then read C/Z directly:
//   ult = !C    uge = C    ugt = C && !Z    ule = !C || Z
// Signed compares first XOR the sign byte with 0x80 (signed order ==
// unsigned order of (v ^ 0x80)), so the same C/Z logic applies to the
// complemented operands.

#[test]
fn icmp_unsigned_i8_materializes_flag_predicates() {
    let m = parse(
        "global x i8\nglobal y i8\nglobal o1 i8\nglobal o2 i8\nglobal o3 i8\nglobal o4 i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r1 = icmp ult i8 %a, %b\n    store i8 %r1 @o1\n    %r2 = icmp uge i8 %a, %b\n    store i8 %r2 @o2\n    %r3 = icmp ugt i8 %a, %b\n    store i8 %r3 @o3\n    %r4 = icmp ule i8 %a, %b\n    store i8 %r4 @o4\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2A,
    // %r1=0x2B, %r2=0x2C, %r3=0x2D, %r4=0x2E.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o1", 0x22),
        ("o2", 0x23),
        ("o3", 0x24),
        ("o4", 0x25),
        ("main::a", 0x29),
        ("main::b", 0x2A),
        ("main::r1", 0x2B),
        ("main::r2", 0x2C),
        ("main::r3", 0x2D),
        ("main::r4", 0x2E),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Compare prefix: %a=0x29, %b=0x2A.
    assert_eq!(
        asm.matches("MOVF 0x2A, W\n    SUBWF 0x29, W").count(),
        4,
        "four compares must share the MOVF b,W; SUBWF a,W prefix:\n{asm}"
    );
    // ult = !C: MOVLW 0; BTFSS STATUS,0; MOVLW 1.
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2B"),
        "ult = !C:\n{asm}"
    );
    // uge = C: MOVLW 0; BTFSC STATUS,0; MOVLW 1.
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2C"),
        "uge = C:\n{asm}"
    );
    // ugt = C && !Z: MOVLW 0; BTFSC C; MOVLW 1; BTFSC Z; MOVLW 0.
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x00\n    MOVWF 0x2D"),
        "ugt = C && !Z:\n{asm}"
    );
    // ule = !C || Z: MOVLW 0; BTFSS C; MOVLW 1; BTFSC Z; MOVLW 1.
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x01\n    MOVWF 0x2E"),
        "ule = !C || Z:\n{asm}"
    );
}

#[test]
fn icmp_signed_i8_complements_sign_bit() {
    let m = parse(
        "global x i8\nglobal y i8\nglobal o1 i8\nglobal o2 i8\nglobal o3 i8\nglobal o4 i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r1 = icmp slt i8 %a, %b\n    store i8 %r1 @o1\n    %r2 = icmp sge i8 %a, %b\n    store i8 %r2 @o2\n    %r3 = icmp sgt i8 %a, %b\n    store i8 %r3 @o3\n    %r4 = icmp sle i8 %a, %b\n    store i8 %r4 @o4\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2A,
    // %r1=0x2B, %r2=0x2C, %r3=0x2D, %r4=0x2E. scratch = fixed 0x70.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o1", 0x22),
        ("o2", 0x23),
        ("o3", 0x24),
        ("o4", 0x25),
        ("main::a", 0x29),
        ("main::b", 0x2A),
        ("main::r1", 0x2B),
        ("main::r2", 0x2C),
        ("main::r3", 0x2D),
        ("main::r4", 0x2E),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Signed prefix: scratch = a ^ 0x80, W = b ^ 0x80, SUBWF scratch,W.
    assert_eq!(
        asm.matches("MOVLW 0x80\n    XORWF 0x29, W\n    MOVWF 0x70\n    MOVLW 0x80\n    XORWF 0x2A, W\n    SUBWF 0x70, W")
            .count(),
        4,
        "signed compares must complement both sign bits before SUBWF:\n{asm}"
    );
    // slt = !C, sge = C, sgt = C && !Z, sle = !C || Z (same materialization
    // as the unsigned forms, driven by the sign-complemented C).
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2B"),
        "slt = !C:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2C"),
        "sge = C:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x00\n    MOVWF 0x2D"),
        "sgt = C && !Z:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x01\n    MOVWF 0x2E"),
        "sle = !C || Z:\n{asm}"
    );
}

#[test]
fn icmp_ne_i8_inverts_eq_materialization() {
    // ne = !Z: the same XOR accumulation as eq, but BTFSS instead of BTFSC
    // so the i1 is 1 exactly when the accumulator is non-zero (a != b).
    let m = parse(
        "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %c = icmp ne i8 %1, 1\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: root frame at 0x25; %1=0x25, %c=0x26.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::c", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %1=0x25, %c=0x26, scratch=0x70. epic-cc#214: %1's own reload for the
    // XOR is redundant (still in W right after Inst::Load's own store to
    // 0x25) and gets elided, so "load a" is no longer its own instruction.
    assert!(asm.contains("XORLW 0x01"), "xor with const b:\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "store xor to scratch:\n{asm}");
    assert!(
        asm.contains("BTFSS STATUS, 2"),
        "ne tests Z inverted (BTFSS):\n{asm}"
    );
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "store d:\n{asm}");
    assert!(
        !asm.contains("BTFSC STATUS, 2"),
        "ne must not use the eq-direction Z test:\n{asm}"
    );
}

#[test]
fn icmp_ult_i16_emits_borrow_chain() {
    // i16 unsigned: the SUBWF borrow chain leaves C = (a16 >= b16); ult =
    // !C. The chain's final Z is a byte-level flag, so C-only predicates
    // never need the equality accumulation.
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp ult i16 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2B,
    // %c=0x2D.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::c", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x29/2A, %b=0x2B/2C, %c=0x2D.
    assert!(
        asm.contains("MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVF 0x2C, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x2C, W\n    SUBWF 0x2A, W"),
        "i16 borrow chain (wrap-correct INCFSZ fold):\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2D"),
        "ult = !C on the chain C:\n{asm}"
    );
}

#[test]
fn icmp_ugt_i16_accumulates_equality_for_z() {
    // ugt = C && !Z at i16: C from the borrow chain, Z from the XOR
    // accumulation (which preserves C), because the chain's final Z only
    // reflects the high byte.
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp ugt i16 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2B,
    // %c=0x2D.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::c", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Chain first (C), then the eq accumulation (Z = a == b), then
    // C && !Z. %a=0x29/2A, %b=0x2B/2C, scratch=0x70.
    assert!(
        asm.contains("MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVF 0x2C, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x2C, W\n    SUBWF 0x2A, W\n    MOVF 0x29, W\n    XORWF 0x2B, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    XORWF 0x2C, W\n    IORWF 0x70, W\n    MOVWF 0x70"),
        "chain then equality accumulation:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x00\n    MOVWF 0x2D"),
        "ugt = C && !Z:\n{asm}"
    );
}

#[test]
fn icmp_slt_i16_complements_sign_bit_in_scratch() {
    // i16 signed: a_hi ^ 0x80 is stored in the scratch byte (the SUBWF file
    // operand), b_hi ^ 0x80 goes in W, and the borrow chain runs against
    // them — C = (a16 >= b16) signed. slt = !C.
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp slt i16 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2B,
    // %c=0x2D.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x22),
        ("out", 0x24),
        ("main::a", 0x29),
        ("main::b", 0x2B),
        ("main::c", 0x2D),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // %a=0x29/2A, %b=0x2B/2C, scratch=0x70.
    assert!(
        asm.contains("MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVLW 0x80\n    XORWF 0x2C, W\n    MOVWF 0x71\n    MOVLW 0x80\n    XORWF 0x2A, W\n    MOVWF 0x70\n    MOVF 0x71, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x71, W\n    SUBWF 0x70, W"),
        "signed i16 chain with a_hi ^ 0x80 in scratch:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x2D"),
        "slt = !C:\n{asm}"
    );
}

#[test]
fn icmp_const_operands_use_literal_paths() {
    // A const RHS is the SUBWF subtrahend via MOVLW; a const LHS uses SUBLW
    // (k - W), since a const can never be read as a file register. Signed
    // consts fold the 0x80 sign complement into the literal.
    let m = parse(
        "global x i8\nglobal y i8\nglobal o1 i8\nglobal o2 i8\nglobal o3 i8\nglobal o4 i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %r1 = icmp ult i8 %a, 9\n    store i8 %r1 @o1\n    %r2 = icmp ult i8 5, %b\n    store i8 %r2 @o2\n    %r3 = icmp slt i8 %a, -1\n    store i8 %r3 @o3\n    %r4 = icmp sge i8 5, %b\n    store i8 %r4 @o4\n    ret void\n",
    );
    // alloc: globals end at 0x26 -> root frame at 0x29; %a=0x29, %b=0x2A,
    // %r1=0x2B, %r2=0x2C, %r3=0x2D, %r4=0x2E.
    let addrs = addrs(&[
        ("x", 0x20),
        ("y", 0x21),
        ("o1", 0x22),
        ("o2", 0x23),
        ("o3", 0x24),
        ("o4", 0x25),
        ("main::a", 0x29),
        ("main::b", 0x2A),
        ("main::r1", 0x2B),
        ("main::r2", 0x2C),
        ("main::r3", 0x2D),
        ("main::r4", 0x2E),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // const RHS: MOVLW 9; SUBWF a,W.
    assert!(
        asm.contains("MOVLW 0x09\n    SUBWF 0x29, W"),
        "const RHS via MOVLW/SUBWF:\n{asm}"
    );
    // const LHS: MOVF b,W; SUBLW 5.
    assert!(
        asm.contains("MOVF 0x2A, W\n    SUBLW 0x05"),
        "const LHS via SUBLW:\n{asm}"
    );
    // signed const RHS: sign complement folded into the literal:
    // MOVLW (0xFF ^ 0x80) = 0x7F; SUBWF scratch,W (the signed file-LHS
    // compares against the complemented a stored in scratch).
    assert!(
        asm.contains("MOVLW 0x7F\n    SUBWF 0x70, W"),
        "signed const RHS folds ^0x80 into the literal:\n{asm}"
    );
    // signed const LHS: MOVLW 0x80; XORWF b,W; SUBLW (5 ^ 0x80) = 0x85.
    assert!(
        asm.contains("MOVLW 0x80\n    XORWF 0x2A, W\n    SUBLW 0x85"),
        "signed const LHS folds ^0x80 into SUBLW:\n{asm}"
    );
}

#[test]
fn cmp_predicates_simulate_correctly() {
    // End-to-end flag check: the exact word sequences isel emits for each
    // i8 predicate, run in pic14_sim with seeded operands, must produce the
    // right i1. RAM: a=0x20, b=0x22, out=0x24; scratch=0x70. This is what
    // validates a wrong flag direction — e.g. ult must come out 1 for
    // a=5,b=9 and 0 for a=9,b=5.
    use pic14_sim::Pic14;
    // Compare prefix: MOVF b,W(0x0822) SUBWF a,W(0x0220).
    let pre = vec![0x0822, 0x0220];
    // ult = !C: MOVLW 0(0x3000) BTFSS C(0x1C03) MOVLW 1(0x3001) MOVWF out(0x00A4).
    let ult = |pre: &[u16]| {
        let mut v = pre.to_vec();
        v.extend([0x3000, 0x1C03, 0x3001, 0x00A4]);
        v
    };
    // uge = C: MOVLW 0 BTFSC C(0x1803) MOVLW 1.
    let uge = |pre: &[u16]| {
        let mut v = pre.to_vec();
        v.extend([0x3000, 0x1803, 0x3001, 0x00A4]);
        v
    };
    // ugt = C && !Z: MOVLW 0 BTFSC C MOVLW 1 BTFSC Z(0x1903) MOVLW 0.
    let ugt = |pre: &[u16]| {
        let mut v = pre.to_vec();
        v.extend([0x3000, 0x1803, 0x3001, 0x1903, 0x3000, 0x00A4]);
        v
    };
    // ule = !C || Z: MOVLW 0 BTFSS C MOVLW 1 BTFSC Z MOVLW 1.
    let ule = |pre: &[u16]| {
        let mut v = pre.to_vec();
        v.extend([0x3000, 0x1C03, 0x3001, 0x1903, 0x3001, 0x00A4]);
        v
    };
    // ne = !Z: MOVF a,W(0x0820) XORWF b,W(0x0622) MOVWF scratch(0x00F0)
    // MOVLW 0 BTFSS Z(0x1D03) MOVLW 1 MOVWF out.
    let ne = || vec![0x0820, 0x0622, 0x00F0, 0x3000, 0x1D03, 0x3001, 0x00A4];
    // Signed prefix: MOVLW 0x80(0x3080) XORWF a,W(0x0620) MOVWF scratch(0x00F0)
    // MOVLW 0x80 XORWF b,W(0x0622) SUBWF scratch,W(0x0270).
    let spre = || vec![0x3080, 0x0620, 0x00F0, 0x3080, 0x0622, 0x0270];
    // slt = !C, sge = C, sgt = C && !Z, sle = !C || Z on the signed prefix.
    let slt = || {
        let mut v = spre();
        v.extend([0x3000, 0x1C03, 0x3001, 0x00A4]);
        v
    };
    let sgt = || {
        let mut v = spre();
        v.extend([0x3000, 0x1803, 0x3001, 0x1903, 0x3000, 0x00A4]);
        v
    };

    // ult: 5 < 9 -> 1; 9 < 5 -> 0.
    {
        let mut p = Pic14::new(ult(&pre));
        p.ram_mut()[0x20] = 5;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ult(5,9) must be 1");
    }
    {
        let mut p = Pic14::new(ult(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 5;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ult(9,5) must be 0");
    }

    // uge: 9 >= 5 -> 1; 5 >= 9 -> 0.
    {
        let mut p = Pic14::new(uge(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 5;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "uge(9,5) must be 1");
    }
    {
        let mut p = Pic14::new(uge(&pre));
        p.ram_mut()[0x20] = 5;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "uge(5,9) must be 0");
    }

    // ugt: 9 > 5 -> 1; 9 > 9 -> 0 (equality must clear Z-driven result).
    {
        let mut p = Pic14::new(ugt(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 5;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ugt(9,5) must be 1");
    }
    {
        let mut p = Pic14::new(ugt(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ugt(9,9) must be 0");
    }

    // ule: 5 <= 9 -> 1; 9 <= 5 -> 0; 9 <= 9 -> 1.
    {
        let mut p = Pic14::new(ule(&pre));
        p.ram_mut()[0x20] = 5;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ule(5,9) must be 1");
    }
    {
        let mut p = Pic14::new(ule(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 5;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ule(9,5) must be 0");
    }
    {
        let mut p = Pic14::new(ule(&pre));
        p.ram_mut()[0x20] = 9;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ule(9,9) must be 1");
    }

    // ne: 5 != 9 -> 1; 5 == 5 -> 0.
    {
        let mut p = Pic14::new(ne());
        p.ram_mut()[0x20] = 5;
        p.ram_mut()[0x22] = 9;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ne(5,9) must be 1");
    }
    {
        let mut p = Pic14::new(ne());
        p.ram_mut()[0x20] = 5;
        p.ram_mut()[0x22] = 5;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ne(5,5) must be 0");
    }

    // slt signed: -1 < 1 -> 1; 1 < -1 -> 0.
    {
        let mut p = Pic14::new(slt());
        p.ram_mut()[0x20] = 0xFF; // a = -1
        p.ram_mut()[0x22] = 0x01; // b = 1
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "slt(-1,1) must be 1");
    }
    {
        let mut p = Pic14::new(slt());
        p.ram_mut()[0x20] = 0x01; // a = 1
        p.ram_mut()[0x22] = 0xFF; // b = -1
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "slt(1,-1) must be 0");
    }

    // sgt signed: 1 > -1 -> 1; -1 > 1 -> 0.
    {
        let mut p = Pic14::new(sgt());
        p.ram_mut()[0x20] = 0x01;
        p.ram_mut()[0x22] = 0xFF;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "sgt(1,-1) must be 1");
    }
    {
        let mut p = Pic14::new(sgt());
        p.ram_mut()[0x20] = 0xFF;
        p.ram_mut()[0x22] = 0x01;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "sgt(-1,1) must be 0");
    }
}

#[test]
fn cmp_i16_simulates_correctly() {
    // i16 flag logic end-to-end. RAM: a=0x20(lo)/0x21(hi), b=0x22/0x23,
    // out=0x24; scratch=0x70.
    use pic14_sim::Pic14;
    // Unsigned i16 chain (issue #1, wrap-correct INCFSZ fold): MOVF b_lo
    // (0x0822) SUBWF a_lo (0x0220) MOVF b_hi (0x0823) BTFSS C (0x1C03)
    // INCFSZ b_hi,W (0x0F23) SUBWF a_hi (0x0221). The INCFSZ skip keeps
    // C = borrow-in when b_hi + borrow wraps to 0x100.
    let u16 = vec![0x0822, 0x0220, 0x0823, 0x1C03, 0x0F23, 0x0221];
    // ult16 = chain + !C materialization.
    let ult16 = {
        let mut v = u16.clone();
        v.extend([0x3000, 0x1C03, 0x3001, 0x00A4]);
        v
    };
    // uge16 = chain + C materialization.
    let uge16 = {
        let mut v = u16.clone();
        v.extend([0x3000, 0x1803, 0x3001, 0x00A4]);
        v
    };
    // ugt16 = chain + eq accumulation (MOVF a_lo 0x0820, XORWF b_lo 0x0622,
    // MOVWF scratch 0x00F0, MOVF a_hi 0x0821, XORWF b_hi 0x0623,
    // IORWF scratch,W 0x0470, MOVWF scratch) + C && !Z materialization.
    let ugt16 = {
        let mut v = u16.clone();
        v.extend([
            0x0820, 0x0622, 0x00F0, 0x0821, 0x0623, 0x0470, 0x00F0, // Z = (a == b)
            0x3000, 0x1803, 0x3001, 0x1903, 0x3000, 0x00A4, // C && !Z
        ]);
        v
    };
    // Signed i16 chain (issue #1): MOVF b_lo(0x0822) SUBWF a_lo(0x0220)
    // MOVLW 0x80(0x3080) XORWF b_hi(0x0623) MOVWF 0x71(0x00F1)
    // MOVLW 0x80(0x3080) XORWF a_hi(0x0621) MOVWF scratch(0x00F0)
    // MOVF 0x71,W(0x0871) BTFSS C(0x1C03) INCFSZ 0x71,W(0x0F71)
    // SUBWF scratch,W(0x0270). The complemented b_hi is folded via the
    // INCFSZ skip so the wrap at b_hi ^ 0x80 = 0xFF keeps C = borrow-in.
    let slt16 = {
        let mut v = vec![
            0x0822, 0x0220, 0x3080, 0x0623, 0x00F1, 0x3080, 0x0621, 0x00F0, 0x0871, 0x1C03, 0x0F71,
            0x0270,
        ];
        v.extend([0x3000, 0x1C03, 0x3001, 0x00A4]); // !C
        v
    };

    // ult16: 0x0105 < 0x0106 -> 1; 0x0106 < 0x0105 -> 0 (high bytes equal,
    // the low-byte borrow must decide).
    {
        let mut p = Pic14::new(ult16.clone());
        p.ram_mut()[0x20] = 0x05; // a_lo
        p.ram_mut()[0x21] = 0x01; // a_hi
        p.ram_mut()[0x22] = 0x06; // b_lo
        p.ram_mut()[0x23] = 0x01; // b_hi
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ult16(0x0105,0x0106) must be 1");
    }
    {
        let mut p = Pic14::new(ult16);
        p.ram_mut()[0x20] = 0x06;
        p.ram_mut()[0x21] = 0x01;
        p.ram_mut()[0x22] = 0x05;
        p.ram_mut()[0x23] = 0x01;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ult16(0x0106,0x0105) must be 0");
    }

    // uge16: 0x0105 >= 0x0105 -> 1 (full equality, C from the chain).
    {
        let mut p = Pic14::new(uge16);
        p.ram_mut()[0x20] = 0x05;
        p.ram_mut()[0x21] = 0x01;
        p.ram_mut()[0x22] = 0x05;
        p.ram_mut()[0x23] = 0x01;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "uge16(0x0105,0x0105) must be 1");
    }

    // ugt16: 0x0106 > 0x0105 -> 1; 0x0105 > 0x0105 -> 0 (the equality
    // accumulation must clear the result even though C is set).
    {
        let mut p = Pic14::new(ugt16.clone());
        p.ram_mut()[0x20] = 0x06;
        p.ram_mut()[0x21] = 0x01;
        p.ram_mut()[0x22] = 0x05;
        p.ram_mut()[0x23] = 0x01;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "ugt16(0x0106,0x0105) must be 1");
    }
    {
        let mut p = Pic14::new(ugt16);
        p.ram_mut()[0x20] = 0x05;
        p.ram_mut()[0x21] = 0x01;
        p.ram_mut()[0x22] = 0x05;
        p.ram_mut()[0x23] = 0x01;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "ugt16(0x0105,0x0105) must be 0");
    }

    // slt16 signed: 0xFFFF (-1) < 0x0001 (1) -> 1; 0x0001 < 0xFFFF -> 0.
    {
        let mut p = Pic14::new(slt16.clone());
        p.ram_mut()[0x20] = 0xFF; // a_lo
        p.ram_mut()[0x21] = 0xFF; // a_hi (-1)
        p.ram_mut()[0x22] = 0x01; // b_lo
        p.ram_mut()[0x23] = 0x00; // b_hi (1)
        p.run(1000);
        assert_eq!(p.ram()[0x24], 1, "slt16(-1,1) must be 1");
    }
    {
        let mut p = Pic14::new(slt16);
        p.ram_mut()[0x20] = 0x01;
        p.ram_mut()[0x21] = 0x00;
        p.ram_mut()[0x22] = 0xFF;
        p.ram_mut()[0x23] = 0xFF;
        p.run(1000);
        assert_eq!(p.ram()[0x24], 0, "slt16(1,-1) must be 0");
    }
}

/// Full end-to-end predicate check: `select()` emits the module's asm, the
/// `asm` crate assembles the *actual emitted text* (labels/equ/org and all)
/// into words, and pic14_sim runs the program against seeded operand
/// globals. This validates that what isel emits — not just hand-encoded
/// words — computes the right i1 for the whole pipeline. `out` is asserted
/// via the fixed global address passed in.
fn sim_run(ir_text: &str, map: &[(&str, u16)], seed: &[(u16, u8)], out: u16) -> u8 {
    use pic14_sim::Pic14;
    let m = parse(ir_text);
    let asm = select(&PIC16F877A, &m, &addrs(map));
    let words = asm::assemble(&asm);
    let mut p = Pic14::new(words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    p.ram()[out as usize]
}

#[test]
fn assembled_cmp_predicates_run_in_sim() {
    // i8: a=0x20, b=0x21, out=0x22; locals 0x25..0x27.
    let ir8 = |pred: &str| {
        format!(
            "global a i8\nglobal b i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %x = load i8 @a\n    %y = load i8 @b\n    %c = icmp {pred} i8 %x, %y\n    store i8 %c @out\n    ret void\n"
        )
    };
    let a8 = [
        ("a", 0x20u16),
        ("b", 0x21),
        ("out", 0x22),
        ("main::x", 0x25),
        ("main::y", 0x26),
        ("main::c", 0x27),
    ];
    for (pred, x, y, want) in [
        ("ult", 5u8, 9u8, 1u8),
        ("ult", 9, 5, 0),
        ("ugt", 9, 5, 1),
        ("ugt", 9, 9, 0),
        ("uge", 9, 5, 1),
        ("uge", 5, 9, 0),
        ("ule", 5, 9, 1),
        ("ule", 9, 5, 0),
        ("ule", 9, 9, 1),
        ("ne", 5, 9, 1),
        ("ne", 5, 5, 0),
        ("slt", 0xFF, 0x01, 1), // -1 < 1
        ("slt", 0x01, 0xFF, 0), // 1 < -1
        ("sgt", 0x01, 0xFF, 1), // 1 > -1
        ("sgt", 0xFF, 0x01, 0), // -1 > 1
        ("sge", 0x01, 0x01, 1),
        ("sle", 0xFF, 0xFF, 1), // -1 <= -1
    ] {
        let got = sim_run(&ir8(pred), &a8, &[(0x20, x), (0x21, y)], 0x22);
        assert_eq!(got, want, "assembled {pred}({x},{y}) must be {want}");
    }

    // i16: a=0x20/21, b=0x22/23, out=0x24; locals 0x28..0x2C.
    let ir16 = |pred: &str| {
        format!(
            "global a i16\nglobal b i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %x = load i16 @a\n    %y = load i16 @b\n    %c = icmp {pred} i16 %x, %y\n    store i8 %c @out\n    ret void\n"
        )
    };
    let a16 = [
        ("a", 0x20u16),
        ("b", 0x22),
        ("out", 0x24),
        ("main::x", 0x28),
        ("main::y", 0x2A),
        ("main::c", 0x2C),
    ];
    for (pred, xlo, xhi, ylo, yhi, want) in [
        ("ult", 0x05, 0x01, 0x06, 0x01, 1u8), // 0x0105 < 0x0106
        ("ult", 0x06, 0x01, 0x05, 0x01, 0),   // 0x0106 < 0x0105
        ("ugt", 0x06, 0x01, 0x05, 0x01, 1),   // 0x0106 > 0x0105
        ("ugt", 0x05, 0x01, 0x05, 0x01, 0),   // equal
        ("uge", 0x05, 0x01, 0x05, 0x01, 1),   // equal
        ("slt", 0xFF, 0xFF, 0x01, 0x00, 1),   // -1 < 1
        ("slt", 0x01, 0x00, 0xFF, 0xFF, 0),   // 1 < -1
    ] {
        let got = sim_run(
            &ir16(pred),
            &a16,
            &[(0x20, xlo), (0x21, xhi), (0x22, ylo), (0x23, yhi)],
            0x24,
        );
        assert_eq!(
            got, want,
            "assembled {pred}16(0x{xhi:02X}{xlo:02X},0x{yhi:02X}{ylo:02X}) must be {want}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1: the i16 borrow-chain wrap bug.
//
// The i8/i16 cmp chains used to fold the borrow with `BTFSS C / ADDLW 1`.
// When the folded byte is 0xFF with a pending borrow, W = 0xFF + 1 wraps to
// 0x00 and sets C = 1, so the SUBWF/SUBLW computes against 0x00 and
// leaves C = 1 — a false "no borrow". For a compare, C IS the answer, so
// the predicate silently flips. The i32 chains were fixed in milestone 12
// with the INCFSZ skip idiom (the skip keeps C = borrow-in, the true
// borrow-out); these tests pin the same wrap discriminators at i16, where
// the naive fold was still in use.
// ---------------------------------------------------------------------------

#[test]
fn icmp_uge_i16_wrap_case_simulates() {
    // The issue's reproducer: 0x0000 >= 0xFF01 must be 0. The low byte
    // borrows (0x00 < 0x01); the high-byte fold then wraps at b_hi = 0xFF
    // + borrow-in — a naive ADDLW 1 fold leaves C = 1 and reports 1.
    let ir = "global b i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %y = load i16 @b\n    %c = icmp uge i16 0, %y\n    store i8 %c @out\n    ret void\n";
    let map = [
        ("b", 0x20u16),
        ("out", 0x24),
        ("main::y", 0x28),
        ("main::c", 0x2C),
    ];
    let got = sim_run(ir, &map, &[(0x20, 0x01), (0x21, 0xFF)], 0x24);
    assert_eq!(
        got, 0,
        "uge16(0x0000, 0xFF01) must be 0 (the issue's reproducer)"
    );
    // The inverse predicate on the same operands: 0x0000 < 0xFF01 -> 1.
    let ir = "global b i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %y = load i16 @b\n    %c = icmp ult i16 0, %y\n    store i8 %c @out\n    ret void\n";
    let got = sim_run(ir, &map, &[(0x20, 0x01), (0x21, 0xFF)], 0x24);
    assert_eq!(got, 1, "ult16(0x0000, 0xFF01) must be 1");

    // File-vs-file: same wrap through the SUBWF chain.
    let ir = "global a i16\nglobal b i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %x = load i16 @a\n    %y = load i16 @b\n    %c = icmp uge i16 %x, %y\n    store i8 %c @out\n    ret void\n";
    let map = [
        ("a", 0x20u16),
        ("b", 0x22),
        ("out", 0x24),
        ("main::x", 0x28),
        ("main::y", 0x2A),
        ("main::c", 0x2C),
    ];
    let got = sim_run(
        ir,
        &map,
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x01), (0x23, 0xFF)],
        0x24,
    );
    assert_eq!(got, 0, "uge16(0x0000, 0xFF01) file-vs-file must be 0");
    // And the non-wrap control: 0xFF01 >= 0xFF01 -> 1 (equality, C set).
    let got = sim_run(
        ir,
        &map,
        &[(0x20, 0x01), (0x21, 0xFF), (0x22, 0x01), (0x23, 0xFF)],
        0x24,
    );
    assert_eq!(got, 1, "uge16(0xFF01, 0xFF01) must be 1");

    // Signed complemented-fold wrap: 0x0000 < 0x7FFF must be 1. The high
    // byte's sign complement maps b_hi = 0x7F to 0xFF; with a borrow-in
    // from the low byte the complemented fold wraps — a fold on the
    // uncomplemented byte would wrap invisibly and report 0.
    let ir = "global a i16\nglobal b i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %x = load i16 @a\n    %y = load i16 @b\n    %c = icmp slt i16 %x, %y\n    store i8 %c @out\n    ret void\n";
    let got = sim_run(
        ir,
        &map,
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0xFF), (0x23, 0x7F)],
        0x24,
    );
    assert_eq!(got, 1, "slt16(0x0000, 0x7FFF) must be 1");
    // Control: 0x7FFF < 0x0000 -> 0 (byte 3 decides, no wrap).
    let got = sim_run(
        ir,
        &map,
        &[(0x20, 0xFF), (0x21, 0x7F), (0x22, 0x00), (0x23, 0x00)],
        0x24,
    );
    assert_eq!(got, 0, "slt16(0x7FFF, 0x0000) must be 0");
}

#[test]
fn sub_const_lhs_wrap_simulates() {
    // The const-LHS sub chain (d = k - a) shares the naive ADDLW 1 fold
    // with the cmp chains; at 4 bytes the corrupted intermediate borrow
    // propagates into wrong values. 0 - 0x0000FF01 = 0xFFFF00FF: byte 1's
    // fold wraps at a_1 = 0xFF + borrow-in, and a naive chain then
    // mis-subtracts bytes 2-3 (giving 0xFF000000).
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 0, %a\n    store i32 %r @out\n    ret void\n";
    let map = [
        ("x", 0x20u16),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::r", 0x34),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x01), (0x21, 0xFF), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0xFF, 0x00, 0xFF, 0xFF],
        "0 - 0x0000FF01 must be 0xFFFF00FF"
    );

    // i16 const-LHS sub: 0 - 0xFF01 = 0x00FF. The value is correct even
    // with the naive fold (the wrap only corrupts C, which the 2-byte
    // chain discards), but the ported idiom must keep it correct.
    let ir = "global x i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %r = sub i16 0, %a\n    store i16 %r @out\n    ret void\n";
    let map = [
        ("x", 0x20u16),
        ("out", 0x22),
        ("main::a", 0x27),
        ("main::r", 0x29),
    ];
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0x01), (0x21, 0xFF)], 0x22, 2);
    assert_eq!(&got[..], &[0xFF, 0x00], "0 - 0xFF01 must be 0x00FF");
}

// ---------------------------------------------------------------------------
// Milestone 7, Task 4: byval / sret call ABI (caller side).
// ---------------------------------------------------------------------------

#[test]
fn byval_call_copies_struct_bytes_into_param_slot() {
    // %2 = call i8 @sum(byval4 %1): the caller copies `size` bytes from the
    // arg's pointer (an alloca slot) into the callee's byval param slot via
    // emit_ptr_load_byte — four MOVF buf+i,W / MOVWF sum::p+i pairs — then
    // CALL, then the retval byte into the destination. Scalar args are
    // unchanged (covered by the earlier call tests).
    let m = parse(
        "global out i8\n\
         fn sum(i8) (p=byval4)\n  block entry:\n\
           %a = load i8 %p\n    ret i8 %a\n\
         fn main(void) ()\n  block entry:\n\
           %1 = alloca 4\n    %2 = call i8 @sum(byval4 %1)\n    store i8 %2 @out\n    ret void\n",
    );
    // out=0x21; main's frame: %1 (alloca, 4 bytes)=0x25, %2=0x29; sum's
    // frame: p (byval, 4 bytes)=0x2B, %a=0x2F.
    let addrs = addrs(&[
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x29),
        ("sum::p", 0x2B),
        ("sum::a", 0x2F),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The size-byte copy: alloca bytes 0x25..0x28 -> param slot 0x2B..0x2E.
    for i in 0..4u16 {
        assert!(
            asm.contains(&format!(
                "MOVF 0x{:02X}, W\n    MOVWF 0x{:02X}",
                0x25 + i,
                0x2B + i
            )),
            "byval byte {i} copy (buf+{i} -> sum::p+{i}):\n{asm}"
        );
    }
    assert!(asm.contains("    CALL sum"), "CALL sum:\n{asm}");
    // Retval copy: fixed retval 0x71 -> %2 (0x29).
    assert!(
        asm.contains("MOVF 0x71, W\n    MOVWF 0x29"),
        "retval copy into %2:\n{asm}"
    );
}

#[test]
fn sret_call_stores_target_address_into_param_slot() {
    // call void @make(sret %1): the callee's sret param slot (2 bytes)
    // holds the target address — MOVLW LOW(addr); MOVWF r; MOVLW HIGH(addr);
    // MOVWF r+1, with the target asserted <= 0xFF (bank-0 FSR reachability).
    let m = parse(
        "fn make(void) (r=sret)\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           %1 = alloca 4\n    call void @make(sret %1)\n    ret void\n",
    );
    // main's frame: %1 (alloca)=0x25; make's frame: r (sret, 2 bytes)=0x2F.
    let addrs = addrs(&[("main::1", 0x25), ("make::r", 0x2F)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW 0x25\n    MOVWF 0x2F\n    MOVLW 0x00\n    MOVWF 0x30"),
        "address store into sret param slot:\n{asm}"
    );
}

#[test]
fn sret_call_with_global_target_stores_global_address() {
    // The sret target may be a global; the callee's sret param slot then
    // holds @g's address.
    let m = parse(
        "global g i8\nfn make(void) (r=sret)\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    call void @make(sret @g)\n    ret void\n",
    );
    let addrs = addrs(&[("g", 0x20), ("make::r", 0x2F)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW 0x20\n    MOVWF 0x2F\n    MOVLW 0x00\n    MOVWF 0x30"),
        "global address store into sret param slot:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "outside GPR space")]
fn panics_on_sret_target_outside_gpr() {
    // M9: the sret target may sit in any bank — the callee reaches it via
    // FSR+IRP — but it must still be a GPR address inside one of the four
    // windows. A target past bank 3 (0x200) fails loudly rather than
    // emitting an address FSR/IRP cannot reach.
    let m = parse(
        "fn make(void) (r=sret)\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           %1 = alloca 4\n    call void @make(sret %1)\n    ret void\n",
    );
    let addrs = addrs(&[("main::1", 0x200), ("make::r", 0x2F)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn sret_banked_target_emits_low_high_store_and_irp_dance() {
    // M9: an sret target in bank 2 (alloca at 0x120). The caller stores
    // BOTH address bytes (MOVLW 0x20; MOVWF r; MOVLW 0x01; MOVWF r+1) and
    // the callee's indirect stores set IRP from the stored HIGH byte
    // (BTFSC r+1,0; BSF STATUS,7; BTFSS r+1,0; BCF STATUS,7) before
    // computing FSR = [r] + k — 0x120 -> IRP=1, FSR=0x20.
    let m = parse(
        "fn make(void) (r=sret)\n  block entry:\n\
           store i8 18 %r\n\
           %p = gep %r +2\n\
           store i16 4660 %p\n\
           ret void\n\
         fn main(void) ()\n  block entry:\n\
           %buf = alloca 4\n\
           call void @make(sret %buf)\n\
           ret void\n",
    );
    // make's frame: r (sret, 2 bytes)=0x2E; main's frame: %buf=0x120.
    let addrs = addrs(&[("make::r", 0x2E), ("main::buf", 0x120)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW 0x20\n    MOVWF 0x2E\n    MOVLW 0x01\n    MOVWF 0x2F"),
        "caller stores LOW then HIGH of the 0x120 target:\n{asm}"
    );
    assert!(
        asm.contains("BTFSC 0x2F, 0\n    BSF STATUS, 7\n    BTFSS 0x2F, 0\n    BCF STATUS, 7"),
        "callee sets IRP from the stored hi byte (0x01 -> IRP=1):\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x2E, W\n    ADDLW 0x00\n    MOVWF FSR"),
        "FSR = [r] + k unchanged:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "crosses window end 0x170")]
fn panics_on_sret_target_crossing_window() {
    // M9: the sret target object must fit entirely inside one GPR window —
    // the callee reaches it through FSR+IRP and a span crossing an SFR hole
    // would silently mis-address, so the caller's window check fails
    // loudly. A 32-byte alloca at 0x160 (the last GPR byte of bank 2 is
    // 0x16F) reaches 0x180, crossing the 0x170 hole. (The plan brief's
    // "alloca at 0x130 size 16" example does not actually cross — 0x130 +
    // 16 = 0x140 fits inside bank 2 — so this uses a base whose span
    // genuinely crosses the window end.)
    let m = parse(
        "fn make(void) (r=sret)\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           %1 = alloca 32\n    call void @make(sret %1)\n    ret void\n",
    );
    let addrs = addrs(&[("main::1", 0x160), ("make::r", 0x2F)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
#[should_panic(expected = "sret arg for a non-sret param")]
fn panics_on_sret_arg_for_non_sret_param() {
    // An sret call arg must target an sret callee param (the byval arm has
    // the mirror check); a mismatch is a phase-3 ABI inconsistency and must
    // fail loudly instead of stashing the address in a scalar slot.
    let m = parse(
        "fn make(void) (p=i8)\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           %1 = alloca 4\n    call void @make(sret %1)\n    ret void\n",
    );
    let addrs = addrs(&[("main::1", 0x25), ("make::p", 0x2F)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn byval_call_sum_pair_simulates() {
    // Caller builds a Pair {i8 a, i16 b} in an alloca (a=0x03 at +0,
    // b=0x1234 at +2), calls sum(byval4): the caller copies the 4 bytes into
    // sum's param slot and the callee reads the copy back through a GEP.
    // Result a + b = 3 + 0x34 = 0x37.
    let ir = "global out i8\n\
         fn sum(i8) (p=byval4)\n  block entry:\n\
           %a = load i8 %p\n\
           %q = gep %p +2\n\
           %b = load i16 %q\n\
           %x = zext i8 %a to i16\n\
           %s = add i16 %x, %b\n\
           %t = trunc i16 %s to i8\n\
           ret i8 %t\n\
         fn main(void) ()\n  block entry:\n\
           %buf = alloca 4\n\
           store i8 3 %buf\n\
           %p2 = gep %buf +2\n\
           store i16 4660 %p2\n\
           %r = call i8 @sum(byval4 %buf)\n\
           store i8 %r @out\n\
           ret void\n";
    let map = [
        ("out", 0x20u16),
        ("main::buf", 0x25),
        ("main::r", 0x29),
        ("sum::p", 0x2B),
        ("sum::a", 0x2F),
        ("sum::b", 0x30),
        ("sum::x", 0x32),
        ("sum::s", 0x34),
        ("sum::t", 0x36),
    ];
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The caller's 4-byte copy: buf(0x25..0x28) -> sum::p(0x2B..0x2E).
    assert!(
        asm.contains(
            "MOVF 0x25, W\n    MOVWF 0x2B\n    MOVF 0x26, W\n    MOVWF 0x2C\n    \
             MOVF 0x27, W\n    MOVWF 0x2D\n    MOVF 0x28, W\n    MOVWF 0x2E"
        ),
        "4-byte byval copy:\n{asm}"
    );
    assert_eq!(sim_run(ir, &map, &[], 0x20), 0x37, "sum(3, 0x1234)");
}

#[test]
fn sret_call_make_simulates() {
    // make() writes the struct fields through the sret pointer (r.a = 0x12,
    // r.b = 0x1234); the caller passes its alloca as the target, then reads
    // the fields back — both bytes of the i16 asserted.
    let ir = "global oa i8\nglobal ob i16\n\
         fn make(void) (r=sret)\n  block entry:\n\
           store i8 18 %r\n\
           %p = gep %r +2\n\
           store i16 4660 %p\n\
           ret void\n\
         fn main(void) ()\n  block entry:\n\
           %buf = alloca 4\n\
           call void @make(sret %buf)\n\
           %a = load i8 %buf\n\
           %q = gep %buf +2\n\
           %b = load i16 %q\n\
           store i8 %a @oa\n\
           store i16 %b @ob\n\
           ret void\n";
    let map = [
        ("oa", 0x20u16),
        ("ob", 0x21),
        ("make::r", 0x25),
        ("main::buf", 0x27),
        ("main::a", 0x2B),
        ("main::b", 0x2C),
    ];
    use pic14_sim::Pic14;
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The address store: main::buf (0x27) -> make::r (0x25/0x26).
    assert!(
        asm.contains("MOVLW 0x27\n    MOVWF 0x25\n    MOVLW 0x00\n    MOVWF 0x26"),
        "sret address store:\n{asm}"
    );
    let words = asm::assemble(&asm);
    let mut p = Pic14::new(words);
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    assert_eq!(p.ram()[0x20], 0x12, "oa = r.a");
    assert_eq!(p.ram()[0x21], 0x34, "ob lo = r.b lo");
    assert_eq!(p.ram()[0x22], 0x12, "ob hi = r.b hi");
}

#[test]
fn sret_call_into_banked_alloca_simulates() {
    // M9 load-bearing: a full sret call whose target alloca sits in bank 2
    // (0x120) or bank 3 (0x1A0). The caller stores LOW+HIGH of the target
    // into the callee's sret slot; the callee writes both struct bytes
    // through the indirect pointer (IRP set from the stored hi byte), and
    // the caller reads them back through FSR — a dynamic GEP keeps the
    // read side on FSR/INDF too, since a direct read of banked RAM would
    // need BANKSEL (a later milestone; the Task-1 SIM tests keep the same
    // FSR-only discipline). Assert both struct bytes: r.a = 0x12,
    // r.b = 0x1234. With the IRP-from-hi-byte path missing, the callee's
    // INDF writes hit 0x20/0x22 (bank 0) and the reads come back 0x00.
    let ir = "global in i8\nglobal oa i8\nglobal ob i16\n\
         fn make(void) (r=sret)\n  block entry:\n\
           store i8 18 %r\n\
           %p = gep %r +2\n\
           store i16 4660 %p\n\
           ret void\n\
         fn main(void) ()\n  block entry:\n\
           %buf = alloca 4\n\
           call void @make(sret %buf)\n\
           %i = load i8 @in\n\
           %pa = gep %buf +0 +1*%i\n\
           %a = load i8 %pa\n\
           %q = gep %buf +2 +1*%i\n\
           %b = load i16 %q\n\
           store i8 %a @oa\n\
           store i16 %b @ob\n\
           ret void\n";
    // in=0x20 (seeded 0), oa=0x21, ob=0x22 (i16); make::r=0x25 (2 bytes);
    // main's frame: %buf=target (bank 2/3), %i=0x29, %a=0x2A, %b=0x2B.
    for (target, name) in [(0x120u16, "bank 2"), (0x1A0, "bank 3")] {
        let map = [
            ("in", 0x20u16),
            ("oa", 0x21),
            ("ob", 0x22),
            ("make::r", 0x25),
            ("main::buf", target),
            ("main::i", 0x29),
            ("main::a", 0x2A),
            ("main::b", 0x2B),
        ];
        use pic14_sim::Pic14;
        let m = parse(ir);
        let asm = select(&PIC16F877A, &m, &addrs(&map));
        let words = asm::assemble(&asm);
        let mut p = Pic14::new(words);
        p.ram_mut()[0x20] = 0; // %i = 0: read the struct at its base
        p.run(200_000);
        assert!(p.halted(), "program must SLEEP-halt ({name}):\n{asm}");
        assert_eq!(p.ram()[0x21], 0x12, "oa = r.a ({name})");
        assert_eq!(p.ram()[0x22], 0x34, "ob lo = r.b lo ({name})");
        assert_eq!(p.ram()[0x23], 0x12, "ob hi = r.b hi ({name})");
    }
}

#[test]
fn byval_call_with_global_arg_simulates() {
    // The s6 f(g) pattern: sum(byval4 @g) — the byval arg is a global
    // struct; the caller copies the 4 bytes @g..@g+3 into sum's param slot
    // and the callee reads the copy.
    let ir = "global g i8\nglobal out i8\n\
         fn sum(i8) (p=byval4)\n  block entry:\n\
           %a = load i8 %p\n\
           %q = gep %p +2\n\
           %b = load i16 %q\n\
           %x = zext i8 %a to i16\n\
           %s = add i16 %x, %b\n\
           %t = trunc i16 %s to i8\n\
           ret i8 %t\n\
         fn main(void) ()\n  block entry:\n\
           %r = call i8 @sum(byval4 @g)\n\
           store i8 %r @out\n\
           ret void\n";
    let map = [
        ("g", 0x20u16),
        ("out", 0x24),
        ("main::r", 0x29),
        ("sum::p", 0x2B),
        ("sum::a", 0x2F),
        ("sum::b", 0x30),
        ("sum::x", 0x32),
        ("sum::s", 0x34),
        ("sum::t", 0x36),
    ];
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x2B"),
        "copy @g byte 0 into sum::p:\n{asm}"
    );
    let seed = [(0x20u16, 3u8), (0x21, 0x00), (0x22, 0x34), (0x23, 0x12)];
    assert_eq!(
        sim_run(ir, &map, &seed, 0x24),
        0x37,
        "sum(g) with g = {{3, 0x1234}}"
    );
}

// ---------------------------------------------------------------------------
// Milestone 8, Task 3: mul/div/rem runtime routine bodies (isel recipes).
// ---------------------------------------------------------------------------
//
// The routine Funcs are injected by legalize: params (`a`/`b`, `num`/`den`),
// one `%__scr = alloca N` entry block, and NO `ret` — isel must emit the
// recipe body (adapted from the machine-verified epicurus PIC16 asm) plus the
// RETURN. Args arrive in `{func}::{param}` slots (emit_call copies them), the
// result goes to the fixed retval slots (0x71/0x72), and working state lives
// in `{func}::__scr` at the Task-2 contract offsets. All slot addresses must
// stay ≤ 0x7F (bank 0, loud) — the loops are skip-sensitive and a BANKSEL
// would change the skip targets.

/// The injected routine signatures (ret, params, `__scr` size), mirroring
/// legalize's injection exactly (the Task-2 contract).
fn routine_sig(name: &str) -> (&'static str, &'static [(&'static str, &'static str)], u16) {
    match name {
        "__mul_u8" => ("i8", &[("a", "i8"), ("b", "i8")], 6),
        "__mul_u16" => ("i16", &[("a", "i16"), ("b", "i16")], 14),
        "__udiv_u8" | "__urem_u8" => ("i8", &[("num", "i8"), ("den", "i8")], 4),
        "__udiv_u16" | "__urem_u16" => ("i16", &[("num", "i16"), ("den", "i16")], 7),
        "__sdiv_i8" | "__srem_i8" => ("i8", &[("num", "i8"), ("den", "i8")], 5),
        "__sdiv_i16" | "__srem_i16" => ("i16", &[("num", "i16"), ("den", "i16")], 7),
        "__shl_u8" | "__lshr_u8" | "__ashr_i8" => ("i8", &[("val", "i8"), ("cnt", "i8")], 3),
        "__shl_u16" | "__lshr_u16" | "__ashr_i16" => ("i16", &[("val", "i16"), ("cnt", "i16")], 4),
        "__mul_u32" => ("i32", &[("a", "i32"), ("b", "i32")], 11),
        "__udiv_u32" | "__urem_u32" => ("i32", &[("num", "i32"), ("den", "i32")], 10),
        "__sdiv_i32" | "__srem_i32" => ("i32", &[("num", "i32"), ("den", "i32")], 12),
        "__shl_u32" | "__lshr_u32" | "__ashr_i32" => ("i32", &[("val", "i32"), ("cnt", "i32")], 2),
        other => panic!("test: unknown routine {other}"),
    }
}

/// Build the module for `name`: `main` loads two globals, calls the routine
/// (the injected Func def is written out exactly as legalize produces it),
/// stores the result. The address map places globals at 0x20.., main's
/// locals at 0x25.., and the routine's params + `__scr` at 0x30.. (i8) /
/// 0x40.. (i16) — all ≤ 0x7F, so the raw emitted asm assembles directly
/// (bank-0 file registers only, pre-banking).
fn routine_module(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global ina {ret}\n\
         global inb {ret}\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load {ret} @ina\n\
             %y = load {ret} @inb\n\
             %r = call {ret} @{name}({ret} %x, {ret} %y)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let wide = ret == "i16";
    let (ina, inb, out, x, y, r) = if wide {
        (0x20u16, 0x22, 0x24, 0x28, 0x2A, 0x2C)
    } else {
        (0x20, 0x21, 0x22, 0x25, 0x26, 0x27)
    };
    let mut map = vec![
        ("ina".to_string(), ina),
        ("inb".to_string(), inb),
        ("out".to_string(), out),
        ("main::x".to_string(), x),
        ("main::y".to_string(), y),
        ("main::r".to_string(), r),
    ];
    let mut base = if wide { 0x40u16 } else { 0x30 };
    for (pn, _) in params {
        map.push((format!("{name}::{pn}"), base));
        base += if wide { 2 } else { 1 };
    }
    map.push((format!("{name}::__scr"), base));
    (ir, map)
}

fn map_refs(map: &[(String, u16)]) -> Vec<(&str, u16)> {
    map.iter().map(|(k, v)| (k.as_str(), *v)).collect()
}

/// Simulate the full emitted asm for a routine module with fixed operand
/// bytes; returns the `n` result bytes at `out`.
fn sim_run_bytes(
    ir_text: &str,
    map: &[(String, u16)],
    seed: &[(u16, u8)],
    out: u16,
    n: usize,
) -> Vec<u8> {
    use pic14_sim::Pic14;
    let m = parse(ir_text);
    let asm = select(&PIC16F877A, &m, &addrs(&map_refs(map)));
    let words = asm::assemble(&asm);
    let mut p = Pic14::new(words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    (0..n).map(|i| p.ram()[out as usize + i]).collect()
}

/// Every routine emits a real body — the label, recipe instructions, and a
/// RETURN (not an empty label that would fall through into the next
/// function). The `pats` are the load-bearing idiom strings at the contract
/// addresses (e.g. `__mul_u8`'s `INCFSZ` carry step at t_hi = __scr+5).
#[test]
fn mul_div_rem_routines_emit_recipe_bodies() {
    let cases: &[(&str, &[&str])] = &[
        (
            "__mul_u8",
            &[
                "BTFSS 0x32, 0",  // bk = __scr+0, multiplier bit test
                "ADDWF 0x34, F",  // r_lo = __scr+2
                "INCFSZ 0x37, W", // t_hi = __scr+5: the carry idiom
                "ADDWF 0x35, F",  // r_hi = __scr+3
                "RLF 0x36, F",    // t_lo = __scr+4, tmp <<= 1
                "RRF 0x32, F",    // bk >>= 1
                "DECFSZ 0x33, F", // cnt = __scr+1, 8 iterations
            ],
        ),
        (
            "__mul_u16",
            &[
                "BTFSS 0x44, 0",  // bk_lo = __scr+0
                "INCFSZ 0x4E, W", // t3 = __scr+10: 32-bit carry idiom
                "ADDWF 0x4A, F",  // r3 = __scr+6
                "RLF 0x4B, F",    // t0 = __scr+7
                "RRF 0x45, F",    // bk_hi = __scr+1
                "DECFSZ 0x46, F", // cnt = __scr+2, 16 iterations
            ],
        ),
        (
            "__udiv_u8",
            &[
                "RLF 0x30, F",    // num <<= 1 (dividend param = quotient accumulator)
                "SUBWF 0x32, F",  // rem_lo = __scr+0
                "ADDLW 0x01",     // borrow fold
                "SUBWF 0x33, F",  // rem_hi = __scr+1
                "BSF 0x30, 0",    // quotient bit into num
                "DECFSZ 0x34, F", // cnt = __scr+2, 8 iterations
            ],
        ),
        (
            "__urem_u8",
            &[
                "RLF 0x30, F",
                "SUBWF 0x32, F",
                "ADDLW 0x01",
                "SUBWF 0x33, F",
                "BSF 0x30, 0", // the loop computes quotient + remainder
                "DECFSZ 0x34, F",
            ],
        ),
        (
            "__udiv_u16",
            &[
                "RLF 0x40, F",    // num_lo <<= 1
                "SUBWF 0x44, F",  // rem_lo = __scr+0
                "INCFSZ 0x43, W", // den_hi + borrow: the borrow idiom
                "SUBWF 0x45, F",  // rem_hi = __scr+1
                "BSF 0x40, 0",    // quotient bit
                "DECFSZ 0x46, F", // cnt = __scr+2, 16 iterations
            ],
        ),
        (
            "__urem_u16",
            &[
                "RLF 0x40, F",
                "SUBWF 0x44, F",
                "INCFSZ 0x43, W",
                "SUBWF 0x45, F",
                "BSF 0x40, 0",
                "DECFSZ 0x46, F",
            ],
        ),
        (
            "__sdiv_i8",
            &[
                "BTFSS 0x30, 7", // num sign test
                "COMF 0x30, F",  // |num| in the param slot
                "BSF 0x32, 1",   // flags = __scr+0, bit1: remainder negate
                "BSF 0x32, 0",   // bit0: quotient negate
                "BTFSS 0x32, 0", // tail: negate the quotient
            ],
        ),
        (
            "__srem_i8",
            &[
                "BTFSS 0x30, 7",
                "COMF 0x30, F",
                "BSF 0x32, 1",
                "BTFSS 0x32, 1", // tail: remainder sign follows dividend
                "COMF 0x33, F",  // rem_lo = __scr+1 negated
            ],
        ),
        (
            "__sdiv_i16",
            &[
                "BTFSS 0x41, 7", // num_hi sign test
                "COMF 0x40, F",  // |num| (16-bit) in place
                "COMF 0x41, F",
                "BSF 0x44, 1", // flags = __scr+0
                "BTFSS 0x44, 0",
            ],
        ),
        (
            "__srem_i16",
            &[
                "BTFSS 0x41, 7",
                "COMF 0x40, F",
                "COMF 0x41, F",
                "BSF 0x44, 1",
                "BTFSS 0x44, 1",
                "COMF 0x45, F", // rem_lo = __scr+1 negated
            ],
        ),
    ];
    for &(name, pats) in cases {
        let (ir, map) = routine_module(name);
        let asm = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        assert!(
            asm.contains(&format!("    CALL {name}")),
            "{name} call:\n{asm}"
        );
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body
            .split("main:")
            .next()
            .expect("main label after routine");
        assert!(
            body.contains("    RETURN"),
            "{name} body must end in RETURN, not fall through:\n{asm}"
        );
        for p in pats {
            assert!(asm.contains(p), "{name} must contain `{p}`:\n{asm}");
        }
        assert!(
            body.contains("INCFSZ") || body.contains("RLF") || body.contains("COMF"),
            "{name} body looks like an empty label:\n{asm}"
        );
    }
}

/// The load-bearing simulation tests: each routine's emitted asm is
/// assembled and run in pic14_sim with fixed inputs; the result bytes are
/// asserted. A wrong carry/borrow idiom or sign-wrapper step flips a result.
#[test]
fn mul_div_rem_routines_simulate_correctly() {
    // (routine, operand bytes lo..hi for a and b, expected result bytes)
    let cases: &[(&str, &[u8], &[u8], &[u8])] = &[
        // unsigned mul: 35*7 = 245; 200*200 lo byte = 0x40 (16-bit product 0x9C40).
        ("__mul_u8", &[35], &[7], &[245]),
        ("__mul_u8", &[200], &[200], &[0x40]),
        // 16-bit mul: 300*7 = 2100 = 0x0834; 0x0105*7 = 0x0723.
        ("__mul_u16", &[0x2C, 0x01], &[0x07, 0x00], &[0x34, 0x08]),
        ("__mul_u16", &[0x05, 0x01], &[0x07, 0x00], &[0x23, 0x07]),
        // unsigned divmod: 200/3 = 66 r 2; 301/7 = 43 r 0.
        ("__udiv_u8", &[200], &[3], &[66]),
        ("__urem_u8", &[200], &[3], &[2]),
        ("__udiv_u16", &[0x2D, 0x01], &[0x07, 0x00], &[0x2B, 0x00]),
        ("__urem_u16", &[0x2D, 0x01], &[0x07, 0x00], &[0x00, 0x00]),
        // signed: -128/-2 = 64; -5%3 = -2 = 0xFE; -19/-3 = 6; -19%3 = -1.
        ("__sdiv_i8", &[0x80], &[0xFE], &[0x40]),
        ("__srem_i8", &[0xFB], &[0x03], &[0xFE]),
        ("__sdiv_i16", &[0xED, 0xFF], &[0xFD, 0xFF], &[0x06, 0x00]),
        ("__srem_i16", &[0xED, 0xFF], &[0x03, 0x00], &[0xFF, 0xFF]),
        // signed: exactly one operand negative — -5/3 = -1 = 0xFF;
        // -19/3 = -6 = 0xFFFA (neg_q = num<0 XOR den<0, both arms).
        ("__sdiv_i8", &[0xFB], &[0x03], &[0xFF]),
        ("__sdiv_i16", &[0xED, 0xFF], &[0x03, 0x00], &[0xFA, 0xFF]),
        // Div-by-zero is LLVM poison (documented, no guard): den = 0 makes
        // every subtract succeed, so the quotient accumulates all-ones
        // (0xFFFF) and the remainder is never reduced — it ends up equal to
        // the dividend (the shifted-out bits accumulate back into rem). Any
        // value is legal; the observed deterministic results are pinned.
        ("__udiv_u16", &[0x05, 0x00], &[0x00, 0x00], &[0xFF, 0xFF]),
        ("__urem_u16", &[0x05, 0x00], &[0x00, 0x00], &[0x05, 0x00]),
    ];
    for &(name, x, y, want) in cases {
        let (ir, map) = routine_module(name);
        let (ret, _, _) = routine_sig(name);
        let wide = ret == "i16";
        let (ina, inb, out) = if wide {
            (0x20, 0x22, 0x24)
        } else {
            (0x20, 0x21, 0x22)
        };
        let mut seed = Vec::new();
        for (i, b) in x.iter().enumerate() {
            seed.push((ina + i as u16, *b));
        }
        for (i, b) in y.iter().enumerate() {
            seed.push((inb + i as u16, *b));
        }
        let got = sim_run_bytes(&ir, &map, &seed, out, want.len());
        assert_eq!(&got[..], want, "{name}({x:?}, {y:?}) must be {want:?}");
    }
}

/// A routine frame straddling banks (params in bank 0, `__scr` moved to
/// bank 1) must fail loudly: a BANKSEL the banking pass would insert
/// between a test and its target, or between the two operands of a carry
/// idiom, would change the skip targets. Loud assert, never a silent
/// miscompile.
#[test]
#[should_panic(expected = "straddle banks")]
fn panics_on_banked_routine_slot() {
    let (ir, mut map) = routine_module("__mul_u8");
    for (k, v) in map.iter_mut() {
        if k == "__mul_u8::__scr" {
            *v = 0xA0; // bank 1 (0x80-0xEF): straddles the bank-0 params
        }
    }
    let _ = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
}

/// A routine slot in a different bank entirely (0x120 is bank 2) must also
/// fail loudly.
#[test]
#[should_panic(expected = "straddle banks")]
fn panics_on_routine_slot_past_ram() {
    let (ir, mut map) = routine_module("__mul_u8");
    for (k, v) in map.iter_mut() {
        if k == "__mul_u8::__scr" {
            *v = 0x120;
        }
    }
    let _ = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
}

/// Issue #6: a routine whose WHOLE frame sits in a non-zero bank is legal;
/// the banking pass selects that bank once at the routine entry and never
/// inserts a BANKSEL between a skip test and its target inside the recipe.
/// The banked asm must assemble and simulate to the same result as the
/// bank-0 layout.
#[test]
fn multi_bank_routine_frame_computes_correctly() {
    // Move the whole __mul_u8 frame (params + scratch) into bank 1; main's
    // locals stay in bank 0. The recipe operands are rewritten to their
    // 7-bit bank-relative forms by the banking pass.
    let (ir, mut map) = routine_module("__mul_u8");
    for (k, v) in map.iter_mut() {
        match k.as_str() {
            "__mul_u8::a" => *v = 0xA0,
            "__mul_u8::b" => *v = 0xA1,
            "__mul_u8::__scr" => *v = 0xA2,
            _ => {}
        }
    }
    let m = parse(&ir);
    let asm = select(&PIC16F877A, &m, &addrs(&map_refs(&map)));
    let banked = banking::assign_banks(&PIC16F877A, &asm);

    // The load-bearing invariant: no BANKSEL may sit between a skip test
    // (BTFSS/BTFSC/INCFSZ/DECFSZ) and the instruction it skips over; that
    // would change the skip target.
    let lines: Vec<&str> = banked.lines().collect();
    for w in lines.windows(2) {
        let t0 = w[0].trim();
        let t1 = w[1].trim();
        let is_skip = t0.starts_with("BTFSS")
            || t0.starts_with("BTFSC")
            || t0.starts_with("INCFSZ")
            || t0.starts_with("DECFSZ");
        let is_banksel = t1.starts_with("BSF STATUS") || t1.starts_with("BCF STATUS");
        assert!(
            !(is_skip && is_banksel),
            "a BANKSEL must never sit between a skip test and its target:\n{banked}"
        );
    }

    let words = asm::assemble(&banked);
    let mut p = pic14_sim::Pic14::new(words);
    p.ram_mut()[0x20] = 35; // global ina (copied into the routine's a slot)
    p.ram_mut()[0x21] = 7; // global inb (copied into the routine's b slot)
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{banked}");
    assert_eq!(
        p.ram()[0x22],
        245,
        "35 * 7 = 245 through the bank-1 routine frame"
    );
}

// ---------------------------------------------------------------------------
// Task 4: shifts — inline const counts + the six variable-count routines.
// ---------------------------------------------------------------------------

/// A module where `main` shifts a loaded value by a **constant** count:
/// `%s = {op} {ty} %a, {count}` (legalize keeps const-count shifts as Bin,
/// isel inlines the fixed RLF/RRF sequence).
fn shift_module(op: &str, ty: &str, count: &str) -> String {
    format!(
        "global x {ty}\nglobal out {ty}\nfn main(void) ()\n  block entry:\n    %a = load {ty} @x\n    %s = {op} {ty} %a, {count}\n    store {ty} %s @out\n    ret void\n"
    )
}

/// i8 map: x=0x20, out=0x21, main::a=0x25, main::s=0x26.
fn shift_map8() -> Vec<(String, u16)> {
    vec![
        ("x".to_string(), 0x20),
        ("out".to_string(), 0x21),
        ("main::a".to_string(), 0x25),
        ("main::s".to_string(), 0x26),
    ]
}

/// i16 map: x=0x20(lo)/0x21(hi), out=0x22/0x23, main::a=0x25/0x26, main::s=0x27/0x28.
fn shift_map16() -> Vec<(String, u16)> {
    vec![
        ("x".to_string(), 0x20),
        ("out".to_string(), 0x22),
        ("main::a".to_string(), 0x25),
        ("main::s".to_string(), 0x27),
    ]
}

/// Inline const-count shifts emit exactly the mandated RLF/RRF sequences:
/// shl = `bcf C; rlf lo; rlf hi` × k; lshr = `bcf C; rrf hi; rrf lo` × k
/// (HIGH byte FIRST — the byte order matters); ashr = C-from-sign + the rrf
/// chain × k; k == 0 is a plain copy; i8 is a single-byte chain.
#[test]
fn inline_const_shifts_emit_rlf_rrf_sequences() {
    // shl i16 %a, 3 -> 3 x (BCF C / RLF lo / RLF hi), no RRF anywhere. The
    // value is copied into the dst slot (main::s = 0x27/0x28) and rotated
    // there, so the RLFs target 0x27/0x28 — never the source slot.
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("shl", "i16", "3")),
        &addrs(&map_refs(&shift_map16())),
    );
    assert_eq!(
        asm.matches("    BCF STATUS, 0").count(),
        3,
        "one BCF per step:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RLF 0x27, F").count(),
        3,
        "lo byte rotated each step:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RLF 0x28, F").count(),
        3,
        "hi byte rotated each step:\n{asm}"
    );
    assert!(!asm.contains("RRF"), "shl must not emit rrf:\n{asm}");

    // lshr i16 %a, 2 -> 2 x (BCF C / RRF hi / RRF lo): the high byte MUST
    // rotate before the low byte, or the shifted-out bit lands in the wrong
    // place.
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("lshr", "i16", "2")),
        &addrs(&map_refs(&shift_map16())),
    );
    assert_eq!(
        asm.matches("    BCF STATUS, 0").count(),
        2,
        "one BCF per step:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF 0x28, F").count(),
        2,
        "hi byte first:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF 0x27, F").count(),
        2,
        "lo byte second:\n{asm}"
    );
    let hi = asm.find("    RRF 0x28, F").expect("hi rrf");
    let lo = asm.find("    RRF 0x27, F").expect("lo rrf");
    assert!(hi < lo, "lshr must shift the high byte first:\n{asm}");
    assert!(!asm.contains("RLF"), "lshr must not emit rlf:\n{asm}");

    // ashr i8 %a, 2 -> C set from the sign bit (BTFSC/BSF + BTFSS/BCF) before
    // each RRF; the rrf chain is a single byte for i8 (dst = main::s = 0x26).
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("ashr", "i8", "2")),
        &addrs(&map_refs(&shift_map8())),
    );
    assert_eq!(
        asm.matches("    RRF 0x26, F").count(),
        2,
        "one rrf per step:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF").count(),
        2,
        "i8 ashr must have no second byte:\n{asm}"
    );
    let btfsc = asm.find("    BTFSC 0x26, 7").expect("sign-bit test");
    let btfss = asm.find("    BTFSS 0x26, 7").expect("sign-bit test 2");
    let rrf = asm.find("    RRF 0x26, F").expect("rrf");
    assert!(
        btfsc < btfss && btfss < rrf,
        "C must be set from the sign bit before each rrf:\n{asm}"
    );

    // shl i16 %a, 0 -> a plain copy (MOVF/MOVWF pairs), no rotation at all.
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("shl", "i16", "0")),
        &addrs(&map_refs(&shift_map16())),
    );
    assert!(
        !asm.contains("RLF") && !asm.contains("RRF"),
        "k=0 must be a plain copy:\n{asm}"
    );
    assert!(asm.contains("    MOVF 0x25, W"), "copy lo:\n{asm}");
    assert!(asm.contains("    MOVWF 0x27"), "store lo:\n{asm}");
    assert!(asm.contains("    MOVF 0x26, W"), "copy hi:\n{asm}");
    assert!(asm.contains("    MOVWF 0x28"), "store hi:\n{asm}");

    // i8 single-byte chains: shl i8 %a, 1 -> one RLF on the only byte;
    // lshr i8 %a, 1 -> one RRF on the only byte.
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("shl", "i8", "1")),
        &addrs(&map_refs(&shift_map8())),
    );
    assert_eq!(
        asm.matches("    RLF 0x26, F").count(),
        1,
        "i8 shl is one byte:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RLF").count(),
        1,
        "i8 shl must have no second byte:\n{asm}"
    );
    let asm = select(
        &PIC16F877A,
        &parse(&shift_module("lshr", "i8", "1")),
        &addrs(&map_refs(&shift_map8())),
    );
    assert_eq!(
        asm.matches("    RRF 0x26, F").count(),
        1,
        "i8 lshr is one byte:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF").count(),
        1,
        "i8 lshr must have no second byte:\n{asm}"
    );
}

/// k >= width is LLVM poison: the result is defined as no value, so a loud
/// panic beats emitting a wrong-but-deterministic result.
#[test]
#[should_panic(expected = "const shift count 16 out of range")]
fn panics_on_inline_shift_count_ge_width_i16() {
    let m = parse(&shift_module("shl", "i16", "16"));
    select(&PIC16F877A, &m, &addrs(&map_refs(&shift_map16())));
}

#[test]
#[should_panic(expected = "const shift count 8 out of range")]
fn panics_on_inline_shift_count_ge_width_i8() {
    let m = parse(&shift_module("lshr", "i8", "8"));
    select(&PIC16F877A, &m, &addrs(&map_refs(&shift_map8())));
}

/// A reg-count shift must never reach isel: legalize rewrites it to the
/// routine call. If one does (legalize regression), panic loudly rather than
/// silently emit anything.
#[test]
#[should_panic(expected = "variable-count")]
fn panics_on_variable_count_shift_reaching_isel() {
    let m = parse(
        "global x i16\nglobal n i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %c = load i16 @n\n    %s = shl i16 %a, %c\n    store i16 %s @out\n    ret void\n",
    );
    select(
        &PIC16F877A,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("n", 0x22),
            ("out", 0x24),
            ("main::a", 0x28),
            ("main::c", 0x2A),
            ("main::s", 0x2C),
        ]),
    );
}

/// The load-bearing inline-shift sims: (5 << 3) >> 1 = 20; ashr of a
/// negative i16 0x8005 >> 2 = 0xE001; i8 ashr 0x80 >> 3 = 0xF0.
#[test]
fn inline_shifts_simulate_correctly() {
    // (5 << 3) >> 1 = 40 >> 1 = 20 = 0x0014 (lo, hi).
    let ir = "global x i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %b = shl i16 %a, 3\n    %r = lshr i16 %b, 1\n    store i16 %r @out\n    ret void\n";
    let map: Vec<(String, u16)> = [
        ("x", 0x20),
        ("out", 0x22),
        ("main::a", 0x25),
        ("main::b", 0x27),
        ("main::r", 0x29),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), *v))
    .collect();
    let got = sim_run_bytes(&ir, &map, &[(0x20, 0x05), (0x21, 0x00)], 0x22, 2);
    assert_eq!(&got[..], &[0x14, 0x00], "(5 << 3) >> 1 must be 20");

    // ashr i16: 0x8005 >> 2 = 0xE001 (sign-fill).
    let ir = shift_module("ashr", "i16", "2");
    let got = sim_run_bytes(&ir, &shift_map16(), &[(0x20, 0x05), (0x21, 0x80)], 0x22, 2);
    assert_eq!(&got[..], &[0x01, 0xE0], "0x8005 >> 2 must be 0xE001");

    // ashr i8: 0x80 >> 3 = 0xF0 (sign-fill).
    let ir = shift_module("ashr", "i8", "3");
    let got = sim_run_bytes(&ir, &shift_map8(), &[(0x20, 0x80)], 0x21, 1);
    assert_eq!(got[0], 0xF0, "0x80 >> 3 must be 0xF0");
}

/// The six shift routines emit real recipe bodies: the label, the count
/// mask (ANDLW width-1), the loop (DECFSZ counter, bounded <= 15), the
/// shift idiom on the `val` param slot, and a RETURN. `pats` are the
/// load-bearing strings at the contract addresses (i8: val=0x30, cnt=0x31,
/// __scr=0x32; i16: val=0x40, cnt=0x42, __scr=0x44).
#[test]
fn shift_routines_emit_recipe_bodies() {
    let cases: &[(&str, &[&str])] = &[
        (
            "__shl_u8",
            &[
                "ANDLW 0x07",   // count & (8-1)
                "MOVWF 0x32",   // __scr::cnt@0 = masked count
                "MOVF 0x32, F", // zero test
                "BTFSC STATUS, 2",
                "RLF 0x30, F",    // val shifts in the param slot
                "DECFSZ 0x32, F", // bounded loop counter
            ],
        ),
        (
            "__shl_u16",
            &[
                "ANDLW 0x0F", // count & (16-1)
                "MOVWF 0x44",
                "CLRF 0x45",   // high byte of the masked count
                "RLF 0x40, F", // val_lo
                "RLF 0x41, F", // val_hi (shl: lo then hi)
                "DECFSZ 0x44, F",
            ],
        ),
        (
            "__lshr_u8",
            &["ANDLW 0x07", "RRF 0x30, F", "DECFSZ 0x32, F"],
        ),
        (
            "__lshr_u16",
            &[
                "ANDLW 0x0F",
                "RRF 0x41, F", // val_hi FIRST
                "RRF 0x40, F", // then val_lo (lshr byte order)
                "DECFSZ 0x44, F",
            ],
        ),
        (
            "__ashr_i8",
            &[
                "ANDLW 0x07",
                "BTFSC 0x30, 7", // C = sign bit
                "BSF STATUS, 0",
                "BTFSS 0x30, 7",
                "BCF STATUS, 0",
                "RRF 0x30, F",
                "DECFSZ 0x32, F",
            ],
        ),
        (
            "__ashr_i16",
            &[
                "ANDLW 0x0F",
                "BTFSC 0x41, 7", // C = sign bit (val_hi)
                "BSF STATUS, 0",
                "BTFSS 0x41, 7",
                "BCF STATUS, 0",
                "RRF 0x41, F",
                "RRF 0x40, F",
                "DECFSZ 0x44, F",
            ],
        ),
    ];
    for &(name, pats) in cases {
        let (ir, map) = routine_module(name);
        let asm = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        assert!(
            asm.contains(&format!("    CALL {name}")),
            "{name} call:\n{asm}"
        );
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body
            .split("main:")
            .next()
            .expect("main label after routine");
        assert!(
            body.contains("    RETURN"),
            "{name} body must end in RETURN, not fall through:\n{asm}"
        );
        for p in pats {
            assert!(asm.contains(p), "{name} must contain `{p}`:\n{asm}");
        }
        assert!(
            body.contains("RLF") || body.contains("RRF"),
            "{name} body looks like an empty label:\n{asm}"
        );
    }
}

/// The load-bearing routine sims: fixed operands seeded in RAM (the count
/// arrives UNMASKED — the "volatile input"), the routine masks to width-1
/// and loops. A wrong byte order, a missing sign-fill, or a wrong mask
/// flips a result.
#[test]
fn shift_routines_simulate_correctly() {
    // (routine, val bytes lo..hi, cnt bytes lo..hi, expected result bytes)
    let cases: &[(&str, &[u8], &[u8], &[u8])] = &[
        // __shl_u8: 5<<1 = 10; 0xFF<<3 = 0xF8 (bits out the top); 5<<0 = 5;
        // masked poison range: 5<<15 == 5<<7 = 0x80 (15 & 7 = 7).
        ("__shl_u8", &[5], &[1], &[10]),
        ("__shl_u8", &[0xFF], &[3], &[0xF8]),
        ("__shl_u8", &[5], &[0], &[5]),
        ("__shl_u8", &[5], &[15], &[0x80]),
        // __shl_u16: 5<<3 = 40; 0x8000<<4 = 0; masked poison range:
        // 5<<17 == 5<<1 = 10; 5<<16 == 5<<0 = 5 (16 & 15 = 0).
        ("__shl_u16", &[0x05, 0x00], &[0x03, 0x00], &[0x28, 0x00]),
        ("__shl_u16", &[0x00, 0x80], &[0x04, 0x00], &[0x00, 0x00]),
        ("__shl_u16", &[0x05, 0x00], &[0x11, 0x00], &[0x0A, 0x00]),
        ("__shl_u16", &[0x05, 0x00], &[0x10, 0x00], &[0x05, 0x00]),
        // __lshr_u8: 0x80>>3 = 0x10; 0xFF>>4 = 0x0F.
        ("__lshr_u8", &[0x80], &[3], &[0x10]),
        ("__lshr_u8", &[0xFF], &[4], &[0x0F]),
        // __lshr_u16: 0x8000>>4 = 0x0800; 0x1234>>8 = 0x0012; masked:
        // 0x8000>>17 == 0x8000>>1 = 0x4000.
        ("__lshr_u16", &[0x00, 0x80], &[0x04, 0x00], &[0x00, 0x08]),
        ("__lshr_u16", &[0x34, 0x12], &[0x08, 0x00], &[0x12, 0x00]),
        ("__lshr_u16", &[0x00, 0x80], &[0x11, 0x00], &[0x00, 0x40]),
        // __ashr_i8: 0x80>>3 = 0xF0 (sign-fill); 0x7F>>2 = 0x1F.
        ("__ashr_i8", &[0x80], &[3], &[0xF0]),
        ("__ashr_i8", &[0x7F], &[2], &[0x1F]),
        // __ashr_i16: 0x8005>>2 = 0xE001; 0x7F00>>4 = 0x07F0; masked:
        // 0x8005>>17 == 0x8005>>1 = 0xC002.
        ("__ashr_i16", &[0x05, 0x80], &[0x02, 0x00], &[0x01, 0xE0]),
        ("__ashr_i16", &[0x00, 0x7F], &[0x04, 0x00], &[0xF0, 0x07]),
        ("__ashr_i16", &[0x05, 0x80], &[0x11, 0x00], &[0x02, 0xC0]),
    ];
    for &(name, x, n, want) in cases {
        let (ir, map) = routine_module(name);
        let (ret, _, _) = routine_sig(name);
        let wide = ret == "i16";
        let (ina, inb, out) = if wide {
            (0x20, 0x22, 0x24)
        } else {
            (0x20, 0x21, 0x22)
        };
        let mut seed = Vec::new();
        for (i, b) in x.iter().enumerate() {
            seed.push((ina + i as u16, *b));
        }
        for (i, b) in n.iter().enumerate() {
            seed.push((inb + i as u16, *b));
        }
        let got = sim_run_bytes(&ir, &map, &seed, out, want.len());
        assert_eq!(&got[..], want, "{name}({x:?} <<>> {n:?}) must be {want:?}");
    }
}

/// The brief's variable-shift pin, made explicit: n comes from RAM (the
/// "volatile input"), the routine masks it to width-1, so the poison-range
/// results are deterministic: x << 17 == x << 1, x << 16 == x, and the same
/// for right shifts (logical and arithmetic).
#[test]
fn variable_count_shifts_mask_wide_counts() {
    // (routine, x_lo, x_hi, n_lo, n_hi, want_lo, want_hi)
    let cases: &[(&str, u8, u8, u8, u8, u8, u8)] = &[
        ("__shl_u16", 0x05, 0x00, 0x11, 0x00, 0x0A, 0x00), // x<<17 == x<<1
        ("__shl_u16", 0x05, 0x00, 0x10, 0x00, 0x05, 0x00), // x<<16 == x
        ("__lshr_u16", 0x00, 0x80, 0x11, 0x00, 0x00, 0x40), // x>>17 == x>>1
        ("__ashr_i16", 0x05, 0x80, 0x11, 0x00, 0x02, 0xC0), // x>>17 == x>>1 (sign-fill)
    ];
    for &(name, xlo, xhi, nlo, nhi, wlo, whi) in cases {
        let (ir, map) = routine_module(name);
        let (ina, inb, out) = (0x20, 0x22, 0x24);
        let got = sim_run_bytes(
            &ir,
            &map,
            &[(ina, xlo), (ina + 1, xhi), (inb, nlo), (inb + 1, nhi)],
            out,
            2,
        );
        assert_eq!(
            &got[..],
            &[wlo, whi],
            "{name} with count 0x{nhi:02X}{nlo:02X} (masked) must be 0x{whi:02X}{wlo:02X}"
        );
    }
}

/// A shift-routine frame straddling banks (val in bank 0, `__scr` moved to
/// bank 1) would need BANKSELs inside the skip-sensitive loop; loud
/// assert, same as the mul/div/rem recipes.
#[test]
#[should_panic(expected = "straddle banks")]
fn panics_on_banked_shift_routine_slot() {
    let (ir, mut map) = routine_module("__shl_u16");
    for (k, v) in map.iter_mut() {
        if k == "__shl_u16::__scr" {
            *v = 0xA0; // bank 1 (0x80-0xEF)
        }
    }
    let _ = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
}

// ---- Milestone 10: const-table PCLATH readers and the page-0 bound ----

/// A const table of `size` bytes: bytes 0..min(size,256) = 0x00..0xFF,
/// bytes 256+n = 0x11+n — distinctive per-byte values, so a wrong
/// chunk/window lands on the wrong RETLW (a readable wrong-answer, not a
/// crash).
fn const_table_global(name: &str, size: usize) -> ir::Global {
    let bytes: Vec<u8> = (0..size)
        .map(|i| {
            if i < 256 {
                i as u8
            } else {
                0x11 + (i - 256) as u8
            }
        })
        .collect();
    ir::Global {
        name: name.into(),
        ty: ir::Ty::I8,
        is_const: true,
        size: size as u16,
        bytes,
        refs: Vec::new(),
        addr: None,
    }
}

/// Patch `parse`'d globals with the given set (the IR text format records
/// scalar `global`/`const` lines only — sizes/bytes come from the C
/// frontend, so tests inject them directly, like `pointer_module`).
fn module_with_globals(ir_text: &str, globals: Vec<ir::Global>) -> ir::Module {
    let mut m = parse(ir_text);
    m.globals = globals;
    m
}

/// Word address of a label in isel-emitted asm: walk the lines the same way
/// the asm crate's pass 1 does (org/labels/instructions only — equ,
/// list/radix, and `.table` lines emit no words; `.align N` pads with NOPs
/// to the next N-word boundary).
fn label_addr(asm: &str, label: &str) -> usize {
    let mut org = 0usize;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        if let Some(l) = line.strip_suffix(':') {
            if l.trim() == label {
                return org;
            }
            continue;
        }
        if line.contains(" equ ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        if line.starts_with(".table ") {
            continue;
        }
        org += 1;
    }
    panic!("label {label} not found");
}

/// Assemble and simulate one module: seed RAM, run to SLEEP halt, return
/// the byte at `out`.
fn sim_run_asm(asm: &str, seed: &[(u16, u8)], out: u16) -> u8 {
    use pic14_sim::Pic14;
    let words = asm::assemble(asm);
    let mut p = Pic14::new(words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    p.ram()[out as usize]
}

#[test]
fn large_const_table_emits_two_entry_reader_and_16bit_caller() {
    // M10: a > 255-byte const table emits two chunked reader entries
    // (`__read_t` for bytes 0..255 at chunk label `t`, `__read_t_hi` for
    // 256..size-1 at the fresh chunk label `t_1`), and the caller computes
    // the in-chunk index (0x71 lo temp) + the chunk bit (0x70 hi temp),
    // CALLing the right entry. The index reg is the 16-bit zext of the byte
    // index (clang's `zext i8 %idx to i16` + `gep @t +k +1*%i`).
    let m = module_with_globals(
        "global in i16\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i16 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 300),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Caller: lo-temp 0x71, hi-temp 0x70, chunk-bit test, both entry CALLs.
    assert!(
        asm.contains("MOVWF 0x71"),
        "lo index temp (retval_lo):\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x70"),
        "hi temp / chunk bit (scratch):\n{asm}"
    );
    assert!(asm.contains("BTFSC 0x70, 0"), "chunk-bit test:\n{asm}");
    assert!(
        asm.contains("    CALL __read_t\n"),
        "chunk-0 entry call:\n{asm}"
    );
    assert!(
        asm.contains("    CALL __read_t_hi"),
        "chunk-1 entry call:\n{asm}"
    );
    assert!(asm.contains("GOTO tmp"), "fresh .hi/.done labels:\n{asm}");
    // Reader entry 0: PCLATH = window of t, computed jump into t. The
    // index (W) is stashed in the fixed scratch byte across the PCLATH set
    // (MOVLW HIGH would clobber it otherwise).
    assert!(
        asm.contains("__read_t:\n    MOVWF 0x70"),
        "chunk-0 reader stashes the index:\n{asm}"
    );
    assert!(asm.contains("MOVLW HIGH(t)"), "chunk-0 PCLATH set:\n{asm}");
    assert!(asm.contains("ADDLW LOW(t)"), "chunk-0 index add:\n{asm}");
    // Reader entry 1: window of the fresh `t_1` chunk label (t + 256).
    assert!(
        asm.contains("__read_t_hi:\n    MOVWF 0x70"),
        "chunk-1 reader stashes the index:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW HIGH(t_1)"),
        "chunk-1 PCLATH set:\n{asm}"
    );
    assert!(asm.contains("ADDLW LOW(t_1)"), "chunk-1 index add:\n{asm}");
    // Window-fit directives: `.align 256` 256-aligns the chunk-0 base and
    // `.table t 300` lets the assembler enforce the window fit loudly.
    assert!(
        asm.contains("    .align 256"),
        "chunked base must be aligned:\n{asm}"
    );
    assert!(
        asm.contains("    .table t 300"),
        "window-fit directive before the base label:\n{asm}"
    );
    // Exactly size RETLWs, split 256 + (size-256) across the two chunks,
    // chunk 1 IMMEDIATELY after chunk 0 (no reader entry between — the
    // chunk-1 reader comes after the whole table).
    assert_eq!(
        asm.matches("RETLW").count(),
        300,
        "one RETLW per byte:\n{asm}"
    );
    let t = asm.find("\nt:").unwrap();
    let t1 = asm.find("\nt_1:").unwrap();
    let hi = asm.find("__read_t_hi:").unwrap();
    let chunk0 = &asm[t..t1];
    assert_eq!(
        chunk0.matches("RETLW").count(),
        256,
        "chunk 0 = 256 bytes:\n{asm}"
    );
    let chunk1 = &asm[t1..hi];
    assert_eq!(
        chunk1.matches("RETLW").count(),
        44,
        "chunk 1 = size-256 bytes:\n{asm}"
    );
    // Reordered layout: chunk 1 sits exactly 256 words after chunk 0 (so
    // both chunk bases have LOW == 0 and the true bound is 511, not 505),
    // and the chunk-1 reader follows the whole table.
    let base = label_addr(&asm, "t");
    assert_eq!(label_addr(&asm, "t_1"), base + 256, "t_1 = t + 256:\n{asm}");
    assert_eq!(base & 0xFF, 0, "chunk-0 base must be 256-aligned:\n{asm}");
    assert!(hi > t1, "chunk-1 reader must follow the table:\n{asm}");
}

#[test]
#[should_panic(expected = "multi-term index into large const table")]
fn multi_term_large_table_index_panics() {
    let m = module_with_globals(
        "global in i16\nglobal j i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i16 @in\n    %j = load i8 @j\n    %p = gep @t +0 +1*%i +2*%j\n\
           %v = load i8 %p\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "j".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 300),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("j", 0x22),
        ("main::i", 0x24),
        ("main::j", 0x26),
        ("main::v", 0x27),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
#[should_panic(expected = "constant index into large const table")]
fn const_only_large_table_index_panics() {
    let m = module_with_globals(
        "const t i8\nfn main(void) ()\n  block entry:\n\
           %p = gep @t +5\n    %v = load i8 %p\n    ret void\n",
        vec![const_table_global("t", 300)],
    );
    let mut addrs = HashMap::new();
    addrs.insert("main::v".to_string(), 0x24u16);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
#[should_panic(expected = "const-table label collision")]
fn panics_when_user_const_collides_with_generated_chunk_label() {
    // M10 fix: a user `const t_1` next to a chunked `const t` would emit a
    // duplicate `t_1:` label — the assembler's symbol insert silently
    // overwrites one of them and the table misreads with no error. Guard:
    // panic loudly at const emission time.
    let m = module_with_globals(
        "global in i16\nglobal out i8\nconst t i8\nconst t_1 i8\nfn main(void) ()\n  block entry:\n\
           %i = load i16 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 300),
            const_table_global("t_1", 1),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
#[should_panic(expected = "const-table label collision")]
fn panics_when_user_const_collides_with_generated_reader_label() {
    // Same guard for the chunked table's generated `__read_t_hi:` reader
    // namespace: a user const named `__read_t_hi` collides with it.
    let m = module_with_globals(
        "global in i16\nglobal out i8\nconst t i8\nconst __read_t_hi i8\nfn main(void) ()\n  block entry:\n\
           %i = load i16 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 300),
            const_table_global("__read_t_hi", 1),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn small_const_table_in_nonzero_window_reads_correctly() {
    // M10 load-bearing: a const table placed past 0x100 (window 1) must be
    // read through its reader's PCLATH set — without it the computed PCL
    // jump would land in window 0 and return a wrong byte. A 231-byte
    // filler table (`aaa_fill` sorts before `table`) pushes `table` past
    // 0x100: layout = goto (1) + __start (4) + main (14) + filler
    // reader/table (6+231) + table reader (6) -> table at 0x106. (231 is the
    // largest filler that still fits its own 256-byte window — base 0x19 +
    // 231 == 0x100 exactly; the assembler's `.table` directive now rejects a
    // filler that crosses. The M11 PCLATH pairs add 6 words to main and
    // `__start` moves to the top (+4), so the filler shrank from M10's 241.)
    //
    let m = module_with_globals(
        "global in i8\nglobal out i8\nconst aaa_fill i8\nconst table i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @table +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("aaa_fill", 231),
            ir::Global {
                name: "table".into(),
                ty: ir::Ty::I8,
                is_const: true,
                size: 4,
                bytes: vec![10, 20, 30, 40],
                addr: None,
                refs: Vec::new(),
            },
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Load-bearing precondition: the table lands in a NONZERO 256-byte
    // window and fits it (LOW + 4 <= 0x100) — a reader without the PCLATH
    // set would jump into window 0 and return the wrong byte.
    let base = label_addr(&asm, "table");
    assert!(
        base >= 0x100,
        "table must sit past 0x100 for the PCLATH set to be load-bearing (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        base & 0xFF <= 0xFC,
        "table must fit its window (base 0x{base:03X}):\n{asm}"
    );
    // in = 1 -> table[1] = 20, read via the window-1 reader.
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 1)], 0x21),
        20,
        "table[1] = 20 through the window-1 reader:\n{asm}"
    );
}

#[test]
fn large_const_table_reads_simulate_correctly() {
    // M10 load-bearing: a 300-byte table split into two 256-byte chunks. A
    // 206-byte filler table (`aaa_fill` sorts first) pushes the main table
    // to exactly 0x100: layout = goto (1) + __start (4) + main (33) + filler
    // reader/table (6+206) + t reader (6) = 0x100, so `.align 256` is a
    // no-op and t sits at 0x100 with chunk label t_1 immediately after chunk
    // 0 at 0x200 and `__read_t_hi` after the table. (The M11 PCLATH pairs
    // add 12 words to main's two reader CALLs and `__start` moves to the top
    // (+4), so the filler shrank from M10's 222.) Runtime reads at idx 2
    // (chunk 0), 256 (chunk-1 first), 299 (chunk-1 last), 290, and the
    // lo+carry case (idx 0xF0 + k 0x20 -> in-chunk 0x10, hi 1 -> table[272])
    // must return the right bytes. Bytes: 0..255 = 0x00..0xFF; 256+n = 0x11+n.
    let globals = || {
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("aaa_fill", 206),
            const_table_global("t", 300),
        ]
    };
    let ir = |k: u8| {
        format!(
            "global in i16\nglobal out i8\nconst aaa_fill i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
             %i = load i16 @in\n    %p = gep @t +{k} +1*%i\n    %v = load i8 %p\n\
             store i8 %v @out\n    ret void\n"
        )
    };
    let map = [
        ("in", 0x20u16),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
    ];
    let asm0 = select(
        &PIC16F877A,
        &module_with_globals(&ir(0), globals()),
        &addrs(&map),
    );
    let base = label_addr(&asm0, "t");
    assert!(
        base & 0xFF == 0,
        "chunk 0 must start 256-aligned for the computed jumps to cover all 300 bytes (base 0x{base:03X}):\n{asm0}"
    );
    // (in, k, expected byte)
    let cases: &[(u16, u8, u8)] = &[
        (2, 0, 0x02),       // chunk 0
        (256, 0, 0x11),     // chunk-1 first byte
        (299, 0, 0x3C),     // chunk-1 last byte (0x11 + 43)
        (290, 0, 0x33),     // chunk-1 (0x11 + 34)
        (0xF0, 0x20, 0x21), // lo 0xF0 + k 0x20 = 0x110 -> in-chunk 0x10, hi 1 -> table[272] = 0x11 + 16
    ];
    for (in_val, k, want) in cases {
        let m = module_with_globals(&ir(*k), globals());
        let asm = select(&PIC16F877A, &m, &addrs(&map));
        let got = sim_run_asm(
            &asm,
            &[(0x20, *in_val as u8), (0x21, (*in_val >> 8) as u8)],
            0x22,
        );
        assert_eq!(
            got, *want,
            "table[{in_val}] with k 0x{k:02X} must read 0x{want:02X}:\n{asm}"
        );
    }
}

#[test]
fn exactly_256_byte_table_uses_chunked_shape_and_assembles() {
    // P3 boundary fix: a 256-byte table is a legal two-chunk table (chunk 0
    // = all 256 bytes, chunk 1 = 0 bytes — empty, and unreachable: indices
    // 0..255 always select chunk 0). The old `size > 256` cut sent it down
    // the single-entry branch, which emits `.table t 256` — and the
    // assembler requires LOW(base) == 0 for size > 255, so a 256-byte table
    // whose natural layout base isn't 256-aligned failed assembly (the
    // layout's alignment, not the table, was the arbiter). With the
    // `>= 256` cut the chunked branch's `.align 256` guarantees LOW == 0
    // regardless of the layout, and the empty chunk 1 (`t_1:` with no
    // RETLWs, `__read_t_hi` immediately after) is emitted but never jumped
    // into — the caller's chunk bit is 0 for every index this table accepts.
    let m = module_with_globals(
        "global in i16\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i16 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I16,
                is_const: false,
                size: 2,
                bytes: vec![0; 2],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 256),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Chunked shape: `.align 256`, `.table t 256`, the `t_1` chunk label,
    // and the `__read_t_hi` entry — a 256-byte table is two chunks, not one.
    assert!(
        asm.contains("    .align 256"),
        "256-byte table must take the chunked branch:\n{asm}"
    );
    assert!(
        asm.contains("    .table t 256"),
        "window-fit directive with the full size:\n{asm}"
    );
    assert!(
        asm.contains("\nt_1:"),
        "empty chunk-1 label must be emitted:\n{asm}"
    );
    assert!(
        asm.contains("__read_t_hi:"),
        "chunk-1 reader entry must be emitted:\n{asm}"
    );
    // 256 RETLWs total, all in chunk 0; chunk 1 is empty (t_1 == t + 256,
    // __read_t_hi immediately after the label).
    assert_eq!(
        asm.matches("RETLW").count(),
        256,
        "one RETLW per byte:\n{asm}"
    );
    let t = asm.find("\nt:").unwrap();
    let t1 = asm.find("\nt_1:").unwrap();
    let hi = asm.find("__read_t_hi:").unwrap();
    assert_eq!(
        &asm[t..t1].matches("RETLW").count(),
        &256,
        "chunk 0 = 256 bytes:\n{asm}"
    );
    assert_eq!(
        &asm[t1..hi].matches("RETLW").count(),
        &0,
        "chunk 1 = 0 bytes:\n{asm}"
    );
    let base = label_addr(&asm, "t");
    assert_eq!(label_addr(&asm, "t_1"), base + 256, "t_1 = t + 256:\n{asm}");
    assert_eq!(base & 0xFF, 0, "chunk-0 base must be 256-aligned:\n{asm}");
    // And it assembles + simulates. The table's natural base (goto + main +
    // reader) is NOT 256-aligned here, so the old single-entry cut would
    // fail assembly with the `.table` window assert — the fix must make the
    // layout alignment irrelevant. Reads land in chunk 0 only (0..255).
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 0x00), (0x21, 0x00)], 0x22),
        0x00,
        "table[0] = 0x00:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 0xFF), (0x21, 0x00)], 0x22),
        0xFF,
        "table[255] = 0xFF:\n{asm}"
    );
}

// ---- Milestone 11: multi-page functions and the PCLATH call discipline ----

/// Build IR text for a function padded to `n` words with self-referencing
/// `%a = add i8 %a, 1` chains (3 words each) — the map needs only one entry.
fn pad_body(n: usize) -> String {
    let mut body = String::new();
    for _ in 0..n {
        body.push_str("    %a = add i8 %a, 1\n");
    }
    body
}

#[test]
fn same_page_call_skips_restore() {
    // main calls helper and both land in page 0: the emitted asm is just
    // `MOVLW PAGE(helper); MOVWF PCLATH; CALL helper` — the restore is
    // skipped because PCLATH already holds the caller's page. This is the
    // two-phase forward-call case too: helper is emitted AFTER main, yet
    // pass A's assignment knows its page by the time pass B emits main's
    // call. `__start` sets PAGE(main) before CALL main and omits the
    // restore (the program ends with SLEEP).
    let m = parse(
        "global a i8\nglobal out i8\n\
         fn helper(i8) (x)\n  block entry:\n\
           %r = add i8 %x, 1\n    ret i8 %r\n\
         fn main(void) ()\n  block entry:\n\
           %1 = load i8 @a\n    %2 = call i8 @helper(i8 %1)\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("helper::x", 0x2A),
        ("helper::r", 0x2B),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW PAGE(helper)\n    MOVWF PCLATH\n    CALL helper\n"),
        "same-page set/CALL with no restore:\n{asm}"
    );
    // No restore pair after the CALL — the set is immediately followed by
    // the next instruction of main's body, never by `MOVLW PAGE(main)`.
    assert!(
        !asm.contains("CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "same-page restore must be skipped:\n{asm}"
    );
    // __start: PAGE(main) set before CALL main, then SLEEP — no restore.
    assert!(
        asm.contains("__start:\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n    CALL main\n    SLEEP"),
        "__start PCLATH set with no restore:\n{asm}"
    );
}

#[test]
fn same_page_const_read_skips_restore() {
    // A const-table read (`CALL __read_t`) gets the same discipline: the
    // caller sets PAGE(__read_t) before the CALL. main and the table both
    // land in page 0, so the restore is skipped — the returned byte is
    // stashed in the fixed scratch (0x70) across the reader's PCLATH write
    // and reloaded into W (no restore pair between).
    let m = module_with_globals(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![const_table_global("t", 4)],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The set goes before the index computation (W = index is the reader's
    // input — the set's MOVLW must not clobber it).
    assert!(
        asm.contains("MOVLW PAGE(__read_t)\n    MOVWF PCLATH"),
        "set before CALL __read_t:\n{asm}"
    );
    // Same-page read: CALL, stash the byte, reload — no restore pair.
    assert!(
        asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVF 0x70, W"),
        "same-page read with no restore, byte preserved:\n{asm}"
    );
    assert!(
        !asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "same-page reader restore must be skipped:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "exceeds a 2048-word page")]
fn panics_on_function_larger_than_a_page() {
    // A function of 2100+ words can never fit one 2048-word page: isel must
    // panic loudly instead of emitting a `.org` that cannot help.
    let m = parse(&format!(
        "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n{}    store i8 %1 @out\n    ret void\n",
        pad_body(700)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::a", 0x26),
    ]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn org_pads_function_across_page_boundary() {
    // main padded to fill page 0's remainder; the greedy assignment emits
    // `.org 0x800` before helper so it lands in page 1.
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n{}    %2 = call i8 @helper(i8 %1)\n    store i8 %2 @out\n    ret void\n\
         fn helper(i8) (x)\n  block entry:\n    %r = add i8 %x, 1\n    ret i8 %r\n",
        pad_body(676)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::a", 0x27),
        ("helper::x", 0x2A),
        ("helper::r", 0x2B),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("    org 0x0800"),
        ".org 0x800 before helper:\n{asm}"
    );
    assert_eq!(
        label_addr(&asm, "helper"),
        0x800,
        "helper in page 1:\n{asm}"
    );
    assert!(
        label_addr(&asm, "main") < 0x800,
        "main stays in page 0:\n{asm}"
    );
    // The main -> helper CALL is cross-page (page 0 -> page 1), so this
    // call KEEPS the restore — the caller's intra-function GOTOs need its
    // own page back.
    assert!(
        asm.contains("CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "cross-page call keeps the restore:\n{asm}"
    );
}

#[test]
fn multi_page_module_runs_in_sim() {
    // M11 load-bearing SIM: main (padded to fill page 0) calls helper which
    // the greedy assignment moves to page 1 via `.org 0x800`. The discipline
    // is exercised in BOTH directions: the cross-page calls (main -> helper,
    // main -> `__read_t` — the table lands in page 1 too) keep the restore —
    // main's post-call GOTO proves PCLATH is back on PAGE(main) — while the
    // same-page calls inside helper (helper -> `__read_t`, helper -> helper2,
    // all of page 1) SKIP the restore — helper's post-call GOTO proves
    // PCLATH still holds page 1, so the elision cannot break the caller's
    // intra-function branches. helper(x) = x == 0 ? 100 : x, plus
    // t[x] + 1 (helper2(x) = x + 1); main: r = helper(in);
    // r2 = r == 0 ? r+1 : r; out = r2 + t[in].
    let mut pad = String::new();
    for _ in 0..665 {
        pad.push_str("    %a = add i8 %a, 1\n");
    }
    let m = module_with_globals(
        &format!(
            "global in i8\nglobal out i8\nconst t i8\n\
             fn main(void) ()\n  block entry:\n{}    %1 = load i8 @in\n    %2 = call i8 @helper(i8 %1)\n\
             \x20   %c = icmp eq i8 %2, 0\n    br i1 %c thenb endb\n\
             block thenb:\n    %3 = add i8 %2, 1\n    br endb\n\
             block endb:\n    %p = phi i8 %2 entry %3 thenb\n\
             \x20   %q = gep @t +0 +1*%1\n    %v = load i8 %q\n    %s = add i8 %p, %v\n\
             \x20   store i8 %s @out\n    ret void\n\
             fn helper(i8) (x)\n  block entry:\n    %c = icmp eq i8 %x, 0\n    br i1 %c then else\n\
             block then:\n    %v = add i8 %x, 100\n    br end\n\
             block else:\n    br end\n\
             block end:\n    %p = phi i8 %v then %x else\n\
             \x20   %q = gep @t +0 +1*%x\n    %b = load i8 %q\n    %w = add i8 %p, %b\n\
             \x20   %w2 = call i8 @helper2(i8 %w)\n    br fin\n\
             block fin:\n    ret i8 %w2\n\
             fn helper2(i8) (x)\n  block entry:\n    %r = add i8 %x, 1\n    ret i8 %r\n",
            pad
        ),
        vec![const_table_global("t", 4)],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::1", 0x26),
        ("main::2", 0x27),
        ("main::c", 0x28),
        ("main::3", 0x29),
        ("main::p", 0x2A),
        ("main::v", 0x2B),
        ("main::s", 0x2C),
        ("helper::x", 0x30),
        ("helper::c", 0x31),
        ("helper::v", 0x32),
        ("helper::p", 0x33),
        ("helper::b", 0x34),
        ("helper::w", 0x35),
        ("helper::w2", 0x36),
        ("helper2::x", 0x3A),
        ("helper2::r", 0x3B),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // Load-bearing preconditions: helper, helper2, and the table land in
    // page 1; main stays in page 0.
    assert!(
        asm.contains("    org 0x0800"),
        ".org 0x800 must be emitted:\n{asm}"
    );
    assert_eq!(
        label_addr(&asm, "helper"),
        0x800,
        "helper must land in page 1:\n{asm}"
    );
    assert!(
        label_addr(&asm, "helper2") >= 0x800 && label_addr(&asm, "helper2") < 0x1000,
        "helper2 must land in page 1:\n{asm}"
    );
    let t = label_addr(&asm, "t");
    assert!(
        t >= 0x800 && t < 0x1000,
        "table must land in page 1 (base 0x{t:03X}):\n{asm}"
    );
    // Same-page calls inside helper (page 1 -> page 1) lose the restore...
    assert!(
        asm.contains("CALL helper2\n    MOVF 0x71, W"),
        "same-page helper2 call must not restore before the retval copy:\n{asm}"
    );
    assert!(
        asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVF 0x70, W"),
        "same-page table read must not restore:\n{asm}"
    );
    // ...while main's cross-page calls (page 0 -> page 1) keep it.
    assert!(
        asm.contains("CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "cross-page helper call keeps the restore:\n{asm}"
    );
    assert!(
        asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "cross-page table read keeps the restore:\n{asm}"
    );
    // Hand-computed results (see the doc comment): helper returns
    // (x == 0 ? 100 : x) + t[x] + 1; out = helper(in) + t[in].
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 0)], 0x21),
        101,
        "in=0: helper=101, t[0]=0:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 1)], 0x21),
        4,
        "in=1: helper=3, t[1]=1:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 2)], 0x21),
        7,
        "in=2: helper=5, t[2]=2:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 3)], 0x21),
        10,
        "in=3: helper=7, t[3]=3:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "beyond page 3")]
fn panics_when_function_would_start_past_page_3() {
    // Four functions each filling most of a page (676 self-adds = 2028 words
    // each) leave the next one needing 0x2000 — past page 3 (device flash).
    // The greedy assignment must panic loudly rather than emit a
    // `.org 0x2000` the assembler would reject.
    let mut ir = String::from("global in i8\n");
    for i in 0..4 {
        ir.push_str(&format!(
            "fn f{i}(void) ()\n  block entry:\n{}    ret void\n",
            pad_body(676)
        ));
    }
    // The four big functions end at 0x1FED (page 3's last word is 0x1FFF);
    // a fifth of 31+ words cannot fit the remainder and would need 0x2000.
    ir.push_str(&format!(
        "fn f4(void) ()\n  block entry:\n{}    ret void\n",
        pad_body(10)
    ));
    let m = parse(&ir);
    let mut pairs: Vec<(String, u16)> = vec![("in".to_string(), 0x20)];
    for i in 0..4 {
        pairs.push((format!("f{i}::a"), 0x25));
    }
    pairs.push(("f4::a".to_string(), 0x25));
    let refs: Vec<(&str, u16)> = pairs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let _ = select(&PIC16F877A, &m, &addrs(&refs));
}

// ---- Milestone 11 final wave: page-aligned anchors and the table pin ----

#[test]
fn exact_boundary_function_stays_anchored_after_elision() {
    // F1 regression: `main`'s pass-A size lands EXACTLY on the page-1
    // boundary (5-word header + 1-word f0 + 1-word f1 + 22 non-pad words +
    // 673 self-add chains = 2048 words), so `helper` would start at 0x800
    // as an exact-boundary CONTINUATION — the strict
    // `addr + size > page_end` overflow check alone emits no pad for it.
    // Pass B elides `main`'s same-page f0 restore (2 words), which would
    // otherwise slide `helper` below the boundary into a straddle: its
    // label would resolve to page 0 while its body straddles into page 1,
    // and its intra-function GOTOs (PAGE(helper) from the label) would
    // misbranch. The page-aligned-start anchor must still emit `.org 0x800`
    // and pin the final addresses to pass A's.
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn f0(void) ()\n  block entry:\n    ret void\n\
         fn f1(void) ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    call void @f0()\n{}\
         \x20   %2 = call i8 @helper(i8 %1)\n    %3 = add i8 %2, 0\n    store i8 %3 @out\n    ret void\n\
         fn helper(i8) (x)\n  block entry:\n    %c = icmp eq i8 %x, 0\n    br i1 %c then else\n\
         block then:\n    %v = add i8 %x, 100\n    br end\n\
         block else:\n    br end\n\
         block end:\n    %p = phi i8 %v then %x else\n    ret i8 %p\n",
        pad_body(673)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::3", 0x27),
        ("main::a", 0x28),
        ("helper::x", 0x30),
        ("helper::c", 0x31),
        ("helper::v", 0x32),
        ("helper::p", 0x33),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The anchor: `.org 0x800` before helper, pinning it to the boundary.
    assert!(
        asm.contains("    org 0x0800"),
        "exact-boundary anchor missing:\n{asm}"
    );
    assert_eq!(
        label_addr(&asm, "helper"),
        0x800,
        "helper must be pinned to 0x800:\n{asm}"
    );
    assert!(
        label_addr(&asm, "main") < 0x800,
        "main stays in page 0:\n{asm}"
    );
    // The elision really happens (main's same-page f0 call loses its
    // restore) — that is the drift that would unanchor helper without the
    // fix. `MOVLW PAGE(main)` appears exactly twice: __start's set and the
    // cross-page helper-call restore; f0's restore would be a third.
    assert!(
        !asm.contains("CALL f0\n    MOVLW PAGE(main)"),
        "same-page f0 restore must be elided:\n{asm}"
    );
    assert_eq!(
        asm.matches("MOVLW PAGE(main)").count(),
        2,
        "exactly __start's set + the helper restore:\n{asm}"
    );
    // The cross-page helper call keeps the restore (page 0 -> page 1).
    assert!(
        asm.contains("CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "cross-page helper call keeps the restore:\n{asm}"
    );
    // Load-bearing sim: helper's intra-function GOTO (after the cross-page
    // CALL) branches in page 1. in == 0 -> helper(0) = 100 (then arm) and
    // in == 5 -> helper(5) = 5 (else arm) — both branch paths exercised
    // with PCLATH = page 1 held across them.
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 0)], 0x21),
        100,
        "in=0 -> helper(0)=100:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 5)], 0x21),
        5,
        "in=5 -> helper(5)=5:\n{asm}"
    );
}

#[test]
fn single_table_elision_drift_folded_by_window_align() {
    // F2 regression, adapted for the window-align fix (issue #138): the
    // reader page map uses the pass-A `table_start`, but pass B emits the
    // tables at the post-elision position. Here the section starts at
    // 0x7FA, so the base (reader + 6 words) sits exactly at 0x800 (page 1);
    // the elided restores pull the natural base back to 0x7FC, where a
    // 200-byte table crosses its window. The align folds it to 0x800 again,
    // matching the map with no `.org` pin, so the caller's restore decision
    // stays exact.
    let m = module_with_globals(
        &format!(
            "global in i8\nglobal out i8\nconst t i8\n\
             fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = call i8 @h0(i8 %1)\n\
             \x20   %p = gep @t +0 +1*%1\n    %v = load i8 %p\n    %s = add i8 %2, %v\n\
             \x20   store i8 %s @out\n    ret void\n\
             fn h0(i8) (x)\n  block entry:\n    %r = xor i8 %x, %x\n    ret i8 %r\n\
             fn last(void) ()\n  block entry:\n{}    %1 = call i8 @h0(i8 7)\n    ret void\n",
            pad_body(665)
        ),
        vec![const_table_global("t", 200)],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::v", 0x27),
        ("main::s", 0x28),
        ("h0::x", 0x2A),
        ("h0::r", 0x2B),
        ("last::1", 0x30),
        ("last::a", 0x31),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The elision drift is real: the reader sits at the post-elision
    // position (0x7F4), not the pass-A 0x7FA. (epic-cc#214 shifted this by
    // one word, epic-cc#217 by one more: h0's `xor i8 %x, %x` now elides
    // its own redundant reload too, same drift mechanism this test
    // exercises, just one word more of it.)
    assert_eq!(
        label_addr(&asm, "__read_t"),
        0x7F4,
        "reader at the post-elision start:\n{asm}"
    );
    assert!(
        !asm.contains("    org 0x07FA"),
        "no pin needed when the window align absorbs the drift:\n{asm}"
    );
    // The window align folds the crossing base back onto the page
    // boundary, so its page matches the reader_pages map (page 1).
    assert!(
        asm.contains("    .align 256\n    .table t 200"),
        "window align must precede the table:\n{asm}"
    );
    let base = label_addr(&asm, "t");
    assert_eq!(base, 0x800, "table base held at the page boundary:\n{asm}");
    assert_eq!(
        base >> 11,
        1,
        "base page matches the reader_pages map:\n{asm}"
    );
    // The elision really happens in the last function (the drift source).
    assert!(
        !asm.contains("CALL h0\n    MOVLW PAGE(last)"),
        "last's same-page h0 restore must be elided:\n{asm}"
    );
    // main's const read is cross-page (0 -> 1) and keeps its restore.
    assert!(
        asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "cross-page table read keeps the restore:\n{asm}"
    );
    // Load-bearing sim: the aligned table reads correctly (h0(x) = 0, so
    // out == t[in]).
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 0)], 0x21),
        0x00,
        "t[0]=0:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 3)], 0x21),
        0x03,
        "t[3]=3:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 199)], 0x21),
        0xC7,
        "t[199]=0xC7:\n{asm}"
    );
}

// ---------------------------------------------------------------------------
// Issue #17: the post-banking page-fit guarantee.
//
// The greedy page assignment (page_step) runs on PASS-A sizes — before the
// banking pass inserts BANKSEL words. A function that fit its page
// pre-banking has no `.org` anchor, so when the BANKSEL growth pushes its
// tail across a page boundary the assembler's backward-`.org` panic never
// fires and the function silently straddles: its label resolves to the
// lower page while its tail sits in the upper page, and its intra-function
// GOTOs (PAGE(<func>) from the label) misbranch. `verify_page_fit` walks
// the FINAL post-banking text and panics loudly on any straddle — exact on
// the final layout, not the pre-banking estimate.
// ---------------------------------------------------------------------------

#[test]
fn banked_growth_packed_into_next_page() {
    // Issue #17 + #12: the bin packing measures POST-banking sizes, so a
    // function whose banking growth would straddle a page boundary is
    // packed into the next page with an anchor instead of straddling.
    // main's body is 2042 words pre-banking (last word 0x7FE, page 0) and
    // grows ~5 BANKSEL words to 2047 — past page 0's 2043-word tail — so
    // it lands in page 1 with a `.org 0x800` anchor, and the post-banking
    // page-fit check passes (no straddle).
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %b = add i8 %1, 0\n{}\
         \x20   store i8 %b @out\n    ret void\n",
        pad_body(678)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::b", 0x26),
        ("main::a", 0xA0), // bank 1: every chain access needs a BANKSEL
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // main is packed into page 1 (its post-banking size doesn't fit page 0).
    assert_eq!(
        label_addr(&asm, "main"),
        0x800,
        "main packed into page 1:\n{asm}"
    );
    let banked = banking::assign_banks(&PIC16F877A, &asm);
    // No panic: the final layout is anchored and page-fit.
    verify_page_fit(&m, &banked);
    // The banked program really runs: out = in + 0 = in.
    assert_eq!(
        sim_run_asm(&banked, &[(0x20, 7)], 0x21),
        7,
        "banked program must run:\n{banked}"
    );
}

#[test]
fn banked_growth_within_page_passes() {
    // Control: the same module with 676 pad chains — pre-banking last
    // occupied word 0x7F8, post-banking (5 BANKSEL words of growth) 0x7FD,
    // still page 0. The check must not false-positive on a banked program
    // that stays in its page, and the banked program must still assemble
    // and run.
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %b = add i8 %1, 0\n{}\
         \x20   store i8 %b @out\n    ret void\n",
        pad_body(676)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::b", 0x26),
        ("main::a", 0xA0),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    let banked = banking::assign_banks(&device::PIC16F877A, &asm);
    // No panic: the extent stays inside page 0.
    verify_page_fit(&m, &banked);
    // The banked program really runs: out = in + 0 = in.
    assert_eq!(
        sim_run_asm(&banked, &[(0x20, 7)], 0x21),
        7,
        "banked program must run:\n{banked}"
    );
}

// ---------------------------------------------------------------------------
// Issue #12: bin packing over measured function sizes.
//
// The greedy next-fit pads to a new page whenever the next function does
// not fit the current page's tail, wasting the tail even when a LATER
// small function could fill it. Bin packing (first-fit in emission order)
// places each function in the lowest-numbered page with room, so a small
// function later in the module fills an earlier page's tail.
// ---------------------------------------------------------------------------

#[test]
fn bin_packing_fills_earlier_page_tail() {
    // f1 fills most of page 0 (~1750 words, ~290 left), f2 is too big for
    // the tail (~1840 words -> page 1), f3 is small (~250 words) and fits
    // the page-0 tail. Greedy: f3 lands in page 1 (the tail is wasted).
    // Bin packing: f3 lands in page 0 at the tail, and the program uses 2
    // pages instead of 3.
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn f1(void) ()\n  block entry:\n{}    ret void\n\
         fn f2(void) ()\n  block entry:\n{}    ret void\n\
         fn f3(void) ()\n  block entry:\n{}    ret void\n\
         fn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    call void @f1()\n    call void @f2()\n    call void @f3()\n    store i8 %1 @out\n    ret void\n",
        pad_body(580), pad_body(610), pad_body(80)
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("f1::a", 0x25),
        ("f2::a", 0x26),
        ("f3::a", 0x27),
        ("main::1", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // f3 fills the page-0 tail: it must land in page 0, not page 1.
    assert!(
        label_addr(&asm, "f3") < 0x800,
        "f3 must fill the page-0 tail:\n{asm}"
    );
    // f2 is too big for the tail and stays in page 1.
    assert!(
        label_addr(&asm, "f2") >= 0x800 && label_addr(&asm, "f2") < 0x1000,
        "f2 must land in page 1:\n{asm}"
    );
    // Only 2 pages are used — no `.org 0x1000` pad.
    assert!(!asm.contains("    org 0x1000"), "no page-2 pad:\n{asm}");
    // Load-bearing sim: main calls f1, f2, f3 (all void) and stores in.
    // The main -> f2 call is cross-page (0 -> 1) and keeps its restore;
    // main -> f1 and main -> f3 are same-page and skip it.
    assert_eq!(sim_run_asm(&asm, &[(0x20, 7)], 0x21), 7, "out = in:\n{asm}");
}

// ---------------------------------------------------------------------------
// Milestone 12, Task 2: i32 arithmetic, compares, casts, shifts + the
// widened 0x71-0x74 retval region.
// ---------------------------------------------------------------------------

/// i32 map: x=0x20 (4 bytes), y=0x24, out=0x28, main::a=0x30, main::b=0x34,
/// main::r=0x38 (all bank-0; the fixed scratch is 0x70, retval 0x71-0x74).
fn i32_map() -> Vec<(&'static str, u16)> {
    vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::r", 0x38),
    ]
}

fn i32_ab_module(op: &str) -> String {
    format!(
        "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = {op} i32 %a, %b\n    store i32 %r @out\n    ret void\n"
    )
}

#[test]
fn add32_reg_reg_emits_four_byte_carry_chain() {
    let m = parse(&i32_ab_module("add"));
    let asm = select(&PIC16F877A, &m, &addrs(&i32_map()));
    // Byte 0 is a plain ADDWF (C = carry out exact); bytes 1-3 fold the
    // carry into a scratch copy of b via INCFSZ's skip — the wrap (b_i =
    // 0xFF + carry) must keep C = carry-in (the true carry-out), so the
    // naive ADDLW 1 fold would corrupt byte i+1. a=0x30, b=0x34, r=0x38.
    assert!(
        asm.contains("    MOVF 0x34, W\n    ADDWF 0x30, W\n    MOVWF 0x38"),
        "byte 0 add:\n{asm}"
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "one carry fold per high byte:\n{asm}"
    );
    assert_eq!(
        asm.matches("    ADDWF 0x39, F").count(),
        1,
        "byte 1 accumulate:\n{asm}"
    );
    assert_eq!(
        asm.matches("    ADDWF 0x3A, F").count(),
        1,
        "byte 2 accumulate:\n{asm}"
    );
    assert_eq!(
        asm.matches("    ADDWF 0x3B, F").count(),
        1,
        "byte 3 accumulate:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x35, W\n    MOVWF 0x70\n    MOVF 0x31, W\n    MOVWF 0x39\n    MOVF 0x70, W\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x70, W\n    ADDWF 0x39, F"),
        "byte 1 carry chain:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x37, W\n    MOVWF 0x70\n    MOVF 0x33, W\n    MOVWF 0x3B\n    MOVF 0x70, W\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x70, W\n    ADDWF 0x3B, F"),
        "byte 3 carry chain:\n{asm}"
    );
}

#[test]
fn add32_reg_const_emits_four_byte_carry_chain() {
    // 0x04030201: each literal byte differs from the carry INCFSZ 0x01, so
    // the k_i MOVLW lines are distinguishable.
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = add i32 %a, 67305985\n    store i32 %r @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::r", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // a=0x30, r=0x38.
    assert!(
        asm.contains("    MOVF 0x30, W\n    ADDLW 0x01\n    MOVWF 0x38"),
        "byte 0 const add:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x31, W\n    MOVWF 0x39\n    MOVLW 0x02\n    MOVWF 0x70\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x70, W\n    ADDWF 0x39, F"),
        "byte 1 const carry chain:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x33, W\n    MOVWF 0x3B\n    MOVLW 0x04\n    MOVWF 0x70\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x70, W\n    ADDWF 0x3B, F"),
        "byte 3 const carry chain:\n{asm}"
    );
}

#[test]
fn sub32_reg_reg_emits_four_byte_borrow_chain() {
    let m = parse(&i32_ab_module("sub"));
    let asm = select(&PIC16F877A, &m, &addrs(&i32_map()));
    // Byte 0 is a plain SUBWF (C = borrow out exact); bytes 1-3 fold the
    // borrow into a scratch copy of b via INCFSZ's skip (the wrap b_i =
    // 0xFF + borrow keeps C = borrow-in = 0, the true borrow-out).
    assert!(
        asm.contains("    MOVF 0x34, W\n    SUBWF 0x30, W\n    MOVWF 0x38"),
        "byte 0 sub:\n{asm}"
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "one borrow fold per high byte:\n{asm}"
    );
    assert_eq!(
        asm.matches("    SUBWF 0x39, F").count(),
        1,
        "byte 1 subtract:\n{asm}"
    );
    assert_eq!(
        asm.matches("    SUBWF 0x3B, F").count(),
        1,
        "byte 3 subtract:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x35, W\n    MOVWF 0x70\n    MOVF 0x31, W\n    MOVWF 0x39\n    MOVF 0x70, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x70, W\n    SUBWF 0x39, F"),
        "byte 1 borrow chain:\n{asm}"
    );
}

#[test]
fn sub32_reg_const_emits_four_byte_borrow_chain() {
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 %a, 67305985\n    store i32 %r @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::r", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    assert!(
        asm.contains("    MOVLW 0x01\n    SUBWF 0x30, W\n    MOVWF 0x38"),
        "byte 0 const sub:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x31, W\n    MOVWF 0x39\n    MOVLW 0x02\n    MOVWF 0x70\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x70, W\n    SUBWF 0x39, F"),
        "byte 1 const borrow chain:\n{asm}"
    );
}

#[test]
fn icmp_ult_i32_emits_four_byte_borrow_chain() {
    let m = parse(
        "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp ult i32 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The 4-byte SUBWF borrow chain: byte 0 plain, bytes 1-3 fold the
    // borrow via INCFSZ on b itself (cmp never writes a/b). The wrap
    // b_i = 0xFF + borrow must keep C = borrow-in = 0 (the true
    // borrow-out); the naive ADDLW 1 fold would leave C = (a_i >= 0) = 1
    // and corrupt the next byte.
    assert!(
        asm.contains("    MOVF 0x34, W\n    SUBWF 0x30, W\n    MOVF 0x35, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x35, W\n    SUBWF 0x31, W"),
        "chain bytes 0-1:\n{asm}"
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x35, W").count(),
        1,
        "byte 1 fold on b_lo+1:\n{asm}"
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x36, W").count(),
        1,
        "byte 2 fold on b_lo+2:\n{asm}"
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x37, W").count(),
        1,
        "byte 3 fold on b_lo+3:\n{asm}"
    );
    // ult = !C materialization.
    assert!(
        asm.contains("    MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x38"),
        "ult = !C:\n{asm}"
    );
}

#[test]
fn icmp_ugt_i32_accumulates_four_byte_equality_for_z() {
    let m = parse(
        "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp ugt i32 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The 4-byte chain first (C), then the 4-byte equality accumulation
    // (Z = a == b) — the chain's final Z reflects only byte 3 — then
    // C && !Z. a=0x30/31/32/33, b=0x34/35/36/37, scratch=0x70.
    assert!(
        asm.contains("    MOVF 0x30, W\n    XORWF 0x34, W\n    MOVWF 0x70\n    MOVF 0x31, W\n    XORWF 0x35, W\n    IORWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x32, W\n    XORWF 0x36, W\n    IORWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x33, W\n    XORWF 0x37, W\n    IORWF 0x70, W\n    MOVWF 0x70"),
        "4-byte equality accumulation:\n{asm}"
    );
    assert!(
        asm.contains("    MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    MOVLW 0x01\n    BTFSC STATUS, 2 ; Z\n    MOVLW 0x00\n    MOVWF 0x38"),
        "ugt = C && !Z:\n{asm}"
    );
}

#[test]
fn icmp_slt_i32_complements_sign_bit_at_byte_3() {
    let m = parse(
        "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp slt i32 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // The sign complement applies ONLY to byte 3 (0x33 / 0x37): the b-side
    // is complemented into the 0x71 temp (free at a compare) and folded via
    // INCFSZ's skip — the complemented fold wraps at b_hi ^ 0x80 = 0xFF
    // (b_hi = 0x7F + borrow), where the skip keeps C = borrow-in = 0, the
    // true borrow-out. The a-side complement goes into scratch (the SUBWF
    // file operand).
    assert!(
        asm.contains("    MOVLW 0x80\n    XORWF 0x37, W\n    MOVWF 0x71\n    MOVLW 0x80\n    XORWF 0x33, W\n    MOVWF 0x70\n    MOVF 0x71, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x71, W\n    SUBWF 0x70, W"),
        "byte 3 signed complement chain:\n{asm}"
    );
    // The low bytes are plain unsigned chain bytes (no 0x80 anywhere else).
    assert!(
        !asm.contains("XORWF 0x30, W"),
        "byte 0 must not be complemented:\n{asm}"
    );
    assert!(
        !asm.contains("XORWF 0x31, W"),
        "byte 1 must not be complemented:\n{asm}"
    );
    assert!(
        !asm.contains("XORWF 0x32, W"),
        "byte 2 must not be complemented:\n{asm}"
    );
    assert!(
        asm.contains("    MOVLW 0x00\n    BTFSS STATUS, 0 ; C\n    MOVLW 0x01\n    MOVWF 0x38"),
        "slt = !C:\n{asm}"
    );
}

#[test]
fn sext_i16_to_i32_sign_fills_bytes_2_and_3() {
    let m = parse(
        "global in i16\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i16 @in\n    %s = sext i16 %v to i32\n    store i32 %s @out\n    ret void\n",
    );
    // in=0x20/0x21, out=0x22..0x25, main::v=0x30/0x31, main::s=0x38..0x3B.
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::s", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // Copy both source bytes, then sign-fill bytes 2-3 from byte 1's bit 7
    // (the SOURCE's sign byte — never byte 3).
    assert!(
        asm.contains("    MOVF 0x30, W\n    MOVWF 0x38\n    MOVF 0x31, W\n    MOVWF 0x39"),
        "sext copies both source bytes:\n{asm}"
    );
    assert!(
        asm.contains("BTFSS 0x31, 7"),
        "sext tests the source hi byte's sign bit:\n{asm}"
    );
    assert!(asm.contains("    MOVLW 0xFF\n"), "negative fill:\n{asm}");
    assert!(asm.contains("    MOVLW 0x00\n"), "positive fill:\n{asm}");
    assert_eq!(
        asm.matches("    MOVWF 0x3A\n    MOVWF 0x3B").count(),
        1,
        "fill bytes 2 and 3:\n{asm}"
    );
}

#[test]
fn zext_i8_to_i32_clears_high_bytes() {
    let m = parse(
        "global in i8\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %z = zext i8 %v to i32\n    store i32 %z @out\n    ret void\n",
    );
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::z", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // epic-cc#214: %v's own reload for the zext is redundant (still in W
    // right after Inst::Load's own store to 0x30) and gets elided.
    assert!(
        asm.contains("    MOVWF 0x38\n    CLRF 0x39\n    CLRF 0x3A\n    CLRF 0x3B"),
        "zext copies byte 0 and clears bytes 1-3:\n{asm}"
    );
}

#[test]
fn trunc_i32_to_i8_copies_low_byte() {
    let m = parse(
        "global in i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %v = load i32 @in\n    %t = trunc i32 %v to i8\n    store i8 %t @out\n    ret void\n",
    );
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::t", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    assert!(
        asm.contains("    MOVF 0x30, W\n    MOVWF 0x38"),
        "trunc copies only the low byte:\n{asm}"
    );
    assert!(
        !asm.contains("MOVF 0x31, W"),
        "trunc must not read the high bytes:\n{asm}"
    );
}

#[test]
fn shl_i32_inline_rotates_four_bytes() {
    // shl i32 %a, 3 -> 3 x (BCF C / RLF lo..hi): the 4 rlf chain rotates the
    // carry up through all four bytes of the dst slot (main::s = 0x38).
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = shl i32 %a, 3\n    store i32 %s @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::s", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    assert_eq!(
        asm.matches("    BCF STATUS, 0").count(),
        3,
        "one BCF per step:\n{asm}"
    );
    for b in [0x38, 0x39, 0x3A, 0x3B] {
        assert_eq!(
            asm.matches(&format!("    RLF 0x{b:02X}, F")).count(),
            3,
            "byte {b:#x} rotated each step:\n{asm}"
        );
    }
    assert!(!asm.contains("RRF"), "shl must not emit rrf:\n{asm}");
}

#[test]
fn ashr_i32_sign_fills_from_byte_3() {
    // ashr i32 %a, 2 -> per step: C from byte 3 bit 7, then RRF hi..lo.
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = ashr i32 %a, 2\n    store i32 %s @out\n    ret void\n",
    );
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::s", 0x38),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    assert_eq!(
        asm.matches("    BTFSC 0x3B, 7").count(),
        2,
        "sign-bit test on byte 3 per step:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF 0x3B, F").count(),
        2,
        "hi byte first:\n{asm}"
    );
    assert_eq!(
        asm.matches("    RRF 0x38, F").count(),
        2,
        "lo byte last:\n{asm}"
    );
}

#[test]
fn i32_call_copies_four_arg_and_retval_bytes() {
    // addm(i32, i32): each arg copies 4 bytes into the callee's param slots,
    // CALL, then the retval region (0x71-0x74) is copied into %3.
    let m = parse(
        "global a i32\nglobal b i32\nglobal out i32\n\
         fn addm(i32) (x, y)\n  block entry:\n\
           %r = add i32 %x, %y\n    ret i32 %r\n\
         fn main(void) ()\n  block entry:\n\
           %1 = load i32 @a\n    %2 = load i32 @b\n\
           %3 = call i32 @addm(i32 %1, i32 %2)\n    store i32 %3 @out\n    ret void\n",
    );
    let map = vec![
        ("a", 0x20),
        ("b", 0x24),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
        ("addm::x", 0x3C),
        ("addm::y", 0x40),
        ("addm::r", 0x44),
    ];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // Arg copies: %1 (0x30..0x33) -> addm::x (0x3C..0x3F), all 4 bytes.
    assert!(
        asm.contains("    MOVF 0x30, W\n    MOVWF 0x3C\n    MOVF 0x31, W\n    MOVWF 0x3D\n    MOVF 0x32, W\n    MOVWF 0x3E\n    MOVF 0x33, W\n    MOVWF 0x3F"),
        "copy %1 into addm::x (4 bytes):\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x37, W\n    MOVWF 0x43"),
        "copy %2 hi byte into addm::y:\n{asm}"
    );
    assert!(asm.contains("    CALL addm"), "CALL addm:\n{asm}");
    // Retval copy: 0x71/0x72/0x73/0x74 -> %3 (0x38..0x3B).
    assert!(
        asm.contains("    MOVF 0x71, W\n    MOVWF 0x38\n    MOVF 0x72, W\n    MOVWF 0x39\n    MOVF 0x73, W\n    MOVWF 0x3A\n    MOVF 0x74, W\n    MOVWF 0x3B"),
        "copy 4 retval bytes into %3:\n{asm}"
    );
}

#[test]
fn ret_i32_copies_value_to_four_retval_bytes() {
    let m = parse(
        "global x i32\nfn main(i32) ()\n  block entry:\n\
           %v = load i32 @x\n    ret i32 %v\n",
    );
    let map = vec![("x", 0x20), ("main::v", 0x25)];
    let asm = select(&PIC16F877A, &m, &addrs(&map));
    // %v = 0x25..0x28, retval = fixed 0x71..0x74.
    assert!(
        asm.contains("    MOVF 0x25, W\n    MOVWF 0x71\n    MOVF 0x26, W\n    MOVWF 0x72\n    MOVF 0x27, W\n    MOVWF 0x73\n    MOVF 0x28, W\n    MOVWF 0x74"),
        "4 retval writes:\n{asm}"
    );
    assert!(asm.contains("    RETURN"), "RETURN:\n{asm}");
}

#[test]
#[should_panic(expected = "const shift count 32 out of range")]
fn panics_on_inline_shift_count_ge_width_i32() {
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = shl i32 %a, 32\n    store i32 %s @out\n    ret void\n",
    );
    select(
        &PIC16F877A,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("out", 0x24),
            ("main::a", 0x30),
            ("main::s", 0x38),
        ]),
    );
}

#[test]
#[should_panic(expected = "sext only supports")]
fn panics_on_sext_i1_to_i32() {
    // i1 -> i32: bit 7 of the i1's storage byte is not the sign of the i1
    // (a 0/1 value), so a bit-7 sign-fill would miscompile — loud panic.
    // (An i1 value arrives as an icmp result, never as a load.)
    let m = parse(
        "global x i8\nglobal y i8\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %b = load i8 @y\n    %v = icmp eq i8 %a, %b\n    %s = sext i1 %v to i32\n    store i32 %s @out\n    ret void\n",
    );
    select(
        &PIC16F877A,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x21),
            ("out", 0x22),
            ("main::a", 0x30),
            ("main::b", 0x31),
            ("main::v", 0x32),
            ("main::s", 0x38),
        ]),
    );
}

// ---- The load-bearing i32 simulations ----

/// i32 SIM map: x=0x20 (4B), y=0x24, out=0x28, main::a=0x30, main::b=0x34,
/// main::r=0x38.
fn sim32_map() -> Vec<(String, u16)> {
    i32_map().iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Convert a `&[(&str, u16)]` map to the `(String, u16)` form `sim_run_bytes` wants.
fn str_map(pairs: &[(&str, u16)]) -> Vec<(String, u16)> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn add32_simulates_with_carry_chain() {
    // 0x12345678 + 5 = 0x1234567D (brief's case: plain add).
    let ir = i32_ab_module("add");
    let got = sim_run_bytes(
        &ir,
        &sim32_map(),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x05),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x7D, 0x56, 0x34, 0x12],
        "0x12345678 + 5 must be 0x1234567D"
    );

    // Carry chain across all four bytes: 0xFFFFFFFF + 1 = 0x00000000.
    let got = sim_run_bytes(
        &ir,
        &sim32_map(),
        &[
            (0x20, 0xFF),
            (0x21, 0xFF),
            (0x22, 0xFF),
            (0x23, 0xFF),
            (0x24, 0x01),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x00, 0x00, 0x00, 0x00],
        "0xFFFFFFFF + 1 must wrap to 0"
    );

    // The wrap case the naive ADDLW 1 fold gets wrong: b_1 = 0xFF with a
    // carry-in from byte 0 — 0x0000FF80 + 0x00000080 = 0x00010000 (a naive
    // chain leaves byte 2 at 0x00, giving 0x0000FF00).
    let got = sim_run_bytes(
        &ir,
        &sim32_map(),
        &[
            (0x20, 0x80),
            (0x21, 0xFF),
            (0x22, 0x00),
            (0x23, 0x00),
            (0x24, 0x80),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x00, 0x00, 0x01, 0x00],
        "0x0000FF80 + 0x80 must be 0x00010000"
    );
}

#[test]
fn sub32_simulates_with_borrow_chain() {
    let ir = i32_ab_module("sub");
    // 0x00010000 - 1 = 0x0000FFFF (borrow through bytes 0-2).
    let got = sim_run_bytes(
        &ir,
        &sim32_map(),
        &[
            (0x20, 0x00),
            (0x21, 0x00),
            (0x22, 0x01),
            (0x23, 0x00),
            (0x24, 0x01),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0xFF, 0xFF, 0x00, 0x00],
        "0x00010000 - 1 must be 0x0000FFFF"
    );

    // The wrap case: b_1 = 0xFF with a borrow-in — 0x0000FF00 - 0x0000FFFF
    // = 0xFFFFFF01 (a naive chain loses the borrow at byte 1).
    let got = sim_run_bytes(
        &ir,
        &sim32_map(),
        &[
            (0x20, 0x00),
            (0x21, 0xFF),
            (0x22, 0x00),
            (0x23, 0x00),
            (0x24, 0xFF),
            (0x25, 0xFF),
            (0x26, 0x00),
            (0x27, 0x00),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x01, 0xFF, 0xFF, 0xFF],
        "0x0000FF00 - 0x0000FFFF must be 0xFFFFFF01"
    );
}

#[test]
fn sub_const_lhs_no_borrow_fold_reloads_the_subtrahend() {
    // const-LHS `d = k - a`: each higher byte preloads k_i into dst and
    // folds the borrow with INCFSZ's skip. On the no-borrow path the skip
    // means SUBWF must find the subtrahend in W, reloaded from scratch
    // after the MOVLW/MOVWF pair (the pair leaves W holding k_i); without
    // it SUBWF computes k_i - k_i = 0, zeroing every no-borrow byte (fuzz
    // corpus seed 128). Bytes 1-3 below all fold no-borrow.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 305419896, %a\n    store i32 %r @out\n    ret void\n";
    let map = [
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::r", 0x34),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x01), (0x21, 0x00), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x77, 0x56, 0x34, 0x12],
        "0x12345678 - 1 must be 0x12345677 (no-borrow bytes 1-3 must not zero)"
    );

    // Mirror corpus shape: borrow-in at byte 1 only (0x120000FF), so
    // bytes 2-3 fold on the no-borrow path again.
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFF), (0x21, 0x00), (0x22, 0x00), (0x23, 0x12)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x79, 0x55, 0x34, 0x00],
        "0x12345678 - 0x120000FF must be 0x00345579"
    );
}

#[test]
fn cmp_i32_simulates_correctly() {
    let ir = "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp ult i32 %a, %b\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x00),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x20),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 1, "(0x12345678 < 0x20000000) must be 1");

    // The wrap case the naive chain gets wrong: b_1 = 0xFF with a
    // borrow-in — 0xFF00FF00 < 0xFF00FF01 must be 1 (a naive chain
    // reports 0, miscomparing at byte 1).
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x00),
            (0x21, 0xFF),
            (0x22, 0x00),
            (0x23, 0xFF),
            (0x24, 0x01),
            (0x25, 0xFF),
            (0x26, 0x00),
            (0x27, 0xFF),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 1, "(0xFF00FF00 < 0xFF00FF01) must be 1");
}

#[test]
fn ugt_i32_z_accumulation_simulates() {
    let ir = "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp ugt i32 %a, %b\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    // 0x12345679 > 0x12345678: byte 3 equal, so only the 4-byte equality
    // accumulation clears Z — a byte-3-only Z would wrongly report 0.
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x79),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x78),
            (0x25, 0x56),
            (0x26, 0x34),
            (0x27, 0x12),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 1, "(0x12345679 > 0x12345678) must be 1");
    // Equal values: C set, Z set -> ugt = 0.
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x78),
            (0x25, 0x56),
            (0x26, 0x34),
            (0x27, 0x12),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 0, "(0x12345678 > 0x12345678) must be 0");
    // a < b: C clear -> ugt = 0.
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x77),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x78),
            (0x25, 0x56),
            (0x26, 0x34),
            (0x27, 0x12),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 0, "(0x12345677 > 0x12345678) must be 0");
}

#[test]
fn slt_i32_complements_sign_byte_and_simulates() {
    // slt = !C with the byte-3 sign complement on both sides. The
    // load-bearing wrap: b_hi = 0x7F (0x7F ^ 0x80 = 0xFF) with a borrow-in
    // from the lower bytes — 0x00000000 < 0x7FFFFFFF must be 1 (a naive
    // complemented fold would corrupt the borrow-out and report 0).
    let ir = "global x i32\nglobal y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %c = icmp slt i32 %a, %b\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x30),
        ("main::b", 0x34),
        ("main::c", 0x38),
    ];
    // 0 < 0x7FFFFFFF -> 1 (borrow chain runs through bytes 0-2, then the
    // complemented high-byte fold wraps at b_hi = 0x7F + borrow).
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x00),
            (0x21, 0x00),
            (0x22, 0x00),
            (0x23, 0x00),
            (0x24, 0xFF),
            (0x25, 0xFF),
            (0x26, 0xFF),
            (0x27, 0x7F),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 1, "(0 < 0x7FFFFFFF) must be 1");
    // INT_MIN < 0 -> 1 (byte 3 decides).
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x00),
            (0x21, 0x00),
            (0x22, 0x00),
            (0x23, 0x80),
            (0x24, 0x00),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x00),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 1, "(INT_MIN < 0) must be 1");
    // 0x7FFFFFFF < INT_MIN -> 0.
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0xFF),
            (0x21, 0xFF),
            (0x22, 0xFF),
            (0x23, 0x7F),
            (0x24, 0x00),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x80),
        ],
        0x28,
        1,
    );
    assert_eq!(got[0], 0, "(0x7FFFFFFF < INT_MIN) must be 0");
}

/// The const-operand i32 compare chains (`emit_cmp_c_file_lhs_wide`'s
/// Val::Const branch and `emit_cmp_c_const_lhs_wide`) are otherwise
/// unreachable — a const operand folds into the SUBWF/SUBLW literal. These
/// pin the same wrap discriminators as the file-vs-file i32 compares, so a
/// wrong fold idiom in either const path flips a result. The IR const
/// literals are decimal (the IR parser accepts no 0x prefix); the seed
/// bytes are hex. Maps: x=0x20, y=0x20, out=0x24, main::a=0x30,
/// main::b=0x30, main::c=0x38.
#[test]
fn const_operand_i32_icmp_simulates_correctly() {
    // --- Const RHS (`icmp ult i32 %a, 5`): the small literal folds into
    // the SUBWF subtrahend; the high byte decides. 0xFFFFFFFB > 5 -> 0.
    let ir = "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %c = icmp ult i32 %a, 5\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFB), (0x21, 0xFF), (0x22, 0xFF), (0x23, 0xFF)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(0xFFFFFFFB < 5) must be 0");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x04), (0x21, 0x00), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(4 < 5) must be 1");

    // --- Const RHS `icmp ult i32 %a, 0x80000000`: byte 3 decides at
    // b_hi = 0x80 (unsigned). 0x7FFFFFFF < 0x80000000 -> 1, equality -> 0.
    let ir = "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %c = icmp ult i32 %a, 2147483648\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFF), (0x21, 0xFF), (0x22, 0xFF), (0x23, 0x7F)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(0x7FFFFFFF < 0x80000000) must be 1");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(0x80000000 < 0x80000000) must be 0");

    // --- Const RHS unsigned borrow wrap: b_1 = 0xFF with a borrow-in from
    // byte 0 (0 < 0x0000FFFF). A naive ADDLW 1 fold would leave C = 1 and
    // mis-report. 0x00010000 > 0x0000FFFF -> 0.
    let ir = "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %c = icmp ult i32 %a, 65535\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(0 < 0x0000FFFF) must be 1");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x01), (0x23, 0x00)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(0x00010000 < 0x0000FFFF) must be 0");

    // --- Const RHS signed complemented-fold wrap: b_hi = 0x7F (^0x80 =
    // 0xFF) with a borrow-in — INT_MIN < 0x7FFFFFFF must be 1 (a fold on
    // the uncomplemented byte would wrap invisibly and report 0).
    let ir = "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %c = icmp slt i32 %a, 2147483647\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(INT_MIN < 0x7FFFFFFF) must be 1");

    // --- Const LHS (`emit_cmp_c_const_lhs_wide`, SUBLW): `icmp slt i32
    // 5, %b` — 5 < INT_MIN -> 0, 5 < -1 -> 0 (byte 3 / sign decide).
    let ir = "global y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %b = load i32 @y\n    %c = icmp slt i32 5, %b\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("y", 0x20),
        ("out", 0x24),
        ("main::b", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(5 < INT_MIN) must be 0");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFF), (0x21, 0xFF), (0x22, 0xFF), (0x23, 0xFF)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(5 < -1) must be 0");
    // Const LHS signed complemented-fold wrap: b_hi = 0x7F (^0x80 = 0xFF)
    // with a borrow-in — 5 < 0x7FFFFFFF must be 1 (the skip keeps
    // C = borrow-in, the true borrow-out).
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFF), (0x21, 0xFF), (0x22, 0xFF), (0x23, 0x7F)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(5 < 0x7FFFFFFF) must be 1");

    // --- Const LHS unsigned borrow wrap: b_1 = 0xFF with a borrow-in from
    // byte 0 — 0x0000FF00 < 0x0000FFFF must be 1; equality -> 0.
    let ir = "global y i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %b = load i32 @y\n    %c = icmp ult i32 65280, %b\n    store i8 %c @out\n    ret void\n";
    let map = vec![
        ("y", 0x20),
        ("out", 0x24),
        ("main::b", 0x30),
        ("main::c", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0xFF), (0x21, 0xFF), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        1,
    );
    assert_eq!(got[0], 1, "(0x0000FF00 < 0x0000FFFF) must be 1");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0xFF), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0, "(0x0000FF00 < 0x0000FF00) must be 0");
}

#[test]
fn sext_i16_and_i8_to_i32_simulate() {
    // i16 0x8000 -> 0xFFFF8000 (sign-fill from byte 1's bit 7).
    let ir = "global in i16\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i16 @in\n    %s = sext i16 %v to i32\n    store i32 %s @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::s", 0x38),
    ];
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0x00), (0x21, 0x80)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0x00, 0x80, 0xFF, 0xFF],
        "sext i16 0x8000 must be 0xFFFF8000"
    );
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0x34), (0x21, 0x12)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0x34, 0x12, 0x00, 0x00],
        "sext i16 0x1234 must be 0x00001234"
    );

    // i8 0x80 -> 0xFFFFFF80 (sign-fill from byte 0's bit 7).
    let ir = "global in i8\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %s = sext i8 %v to i32\n    store i32 %s @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::s", 0x38),
    ];
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0x80)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0x80, 0xFF, 0xFF, 0xFF],
        "sext i8 0x80 must be 0xFFFFFF80"
    );
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0x7F)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0x7F, 0x00, 0x00, 0x00],
        "sext i8 0x7F must be 0x0000007F"
    );
}

#[test]
fn zext_and_trunc_i32_simulate() {
    // zext i8 0xFF -> 0x000000FF; zext i16 0xFFFF -> 0x0000FFFF.
    let ir = "global in i8\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %z = zext i8 %v to i32\n    store i32 %z @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::z", 0x38),
    ];
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0xFF)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0xFF, 0x00, 0x00, 0x00],
        "zext i8 0xFF must be 0x000000FF"
    );

    let ir = "global in i16\nglobal out i32\nfn main(void) ()\n  block entry:\n    %v = load i16 @in\n    %z = zext i16 %v to i32\n    store i32 %z @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x22),
        ("main::v", 0x30),
        ("main::z", 0x38),
    ];
    let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, 0xFF), (0x21, 0xFF)], 0x22, 4);
    assert_eq!(
        &got[..],
        &[0xFF, 0xFF, 0x00, 0x00],
        "zext i16 0xFFFF must be 0x0000FFFF"
    );

    // trunc i32 0x12345678 -> i8 0x78 and -> i16 0x5678.
    let ir = "global in i32\nglobal out8 i8\nglobal out16 i16\nfn main(void) ()\n  block entry:\n    %v = load i32 @in\n    %t8 = trunc i32 %v to i8\n    store i8 %t8 @out8\n    %t16 = trunc i32 %v to i16\n    store i16 %t16 @out16\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out8", 0x24),
        ("out16", 0x26),
        ("main::v", 0x30),
        ("main::t8", 0x38),
        ("main::t16", 0x3A),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x78), (0x21, 0x56), (0x22, 0x34), (0x23, 0x12)],
        0x24,
        1,
    );
    assert_eq!(got[0], 0x78, "trunc i32 -> i8 must give 0x78");
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x78), (0x21, 0x56), (0x22, 0x34), (0x23, 0x12)],
        0x26,
        2,
    );
    assert_eq!(&got[..], &[0x78, 0x56], "trunc i32 -> i16 must give 0x5678");
}

#[test]
fn trunc_to_i1_keeps_only_bit_0() {
    // i1 is consumed as "the whole byte is nonzero", so a trunc to i1 has to
    // drop the truncated-away bits: bit 0 of 0x02 is clear, so it is false.
    let ir = "global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    %b = trunc i8 %v to i1\n    br i1 %b then else\n  block then:\n    store i8 1 @out\n    ret void\n  block else:\n    store i8 0 @out\n    ret void\n";
    let map = vec![
        ("in", 0x20),
        ("out", 0x21),
        ("main::v", 0x30),
        ("main::b", 0x31),
    ];
    for (input, want) in [(0x02u8, 0x00u8), (0x03, 0x01), (0x01, 0x01), (0x00, 0x00)] {
        let got = sim_run_bytes(ir, &str_map(&map), &[(0x20, input)], 0x21, 1);
        assert_eq!(got[0], want, "trunc i8 0x{input:02X} to i1");
    }
}

#[test]
fn inline_i32_shifts_simulate() {
    // shl: 0x12345678 << 3 = 0x91A2B3C0.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = shl i32 %a, 3\n    store i32 %s @out\n    ret void\n";
    let map = vec![
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x30),
        ("main::s", 0x38),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x78), (0x21, 0x56), (0x22, 0x34), (0x23, 0x12)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0xC0, 0xB3, 0xA2, 0x91],
        "0x12345678 << 3 must be 0x91A2B3C0"
    );

    // lshr: 0x80000000 >> 2 = 0x20000000 (logical: the vacated top bits
    // are 0, not the sign).
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = lshr i32 %a, 2\n    store i32 %s @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x00, 0x00, 0x00, 0x20],
        "0x80000000 >> 2 must be 0x20000000"
    );

    // ashr: 0x80000000 >> 2 = 0xE0000000 (the brief's case — sign-fill from
    // byte 3).
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = ashr i32 %a, 2\n    store i32 %s @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x00, 0x00, 0x00, 0xE0],
        "0x80000000 >> 2 (ashr) must be 0xE0000000"
    );

    // ashr: 0x80000000 >> 4 = 0xF8000000 (sign-fill from byte 3).
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %s = ashr i32 %a, 4\n    store i32 %s @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x80)],
        0x24,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x00, 0x00, 0x00, 0xF8],
        "0x80000000 >> 4 must be 0xF8000000"
    );
}

#[test]
fn commutative_i32_binops_simulate() {
    // and/or/xor at i32 ride the byte-generic emit_commutative; the
    // dispatch arms are new (Task 2), so exercise all three.
    let ir = "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = and i32 %a, %b\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &sim32_map(),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x0F),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x80),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x08, 0x00, 0x00, 0x00],
        "0x12345678 & 0x8000000F must be 0x00000008"
    );

    let ir = "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = or i32 %a, %b\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &sim32_map(),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x0F),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x80),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x7F, 0x56, 0x34, 0x92],
        "0x12345678 | 0x8000000F must be 0x9234567F"
    );

    let ir = "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = xor i32 %a, %b\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &sim32_map(),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x0F),
            (0x25, 0x00),
            (0x26, 0x00),
            (0x27, 0x80),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x77, 0x56, 0x34, 0x92],
        "0x12345678 ^ 0x8000000F must be 0x92345677"
    );
}

#[test]
fn i32_call_with_four_byte_param_and_return_simulates() {
    // addm(0x12345678, 5) = 0x1234567D: 4-byte args into the callee's param
    // slots, 4-byte add, 4-byte retval through 0x71-0x74.
    let ir = "global a i32\nglobal b i32\nglobal out i32\n\
              fn addm(i32) (x, y)\n  block entry:\n\
                %r = add i32 %x, %y\n    ret i32 %r\n\
              fn main(void) ()\n  block entry:\n\
                %1 = load i32 @a\n    %2 = load i32 @b\n\
                %3 = call i32 @addm(i32 %1, i32 %2)\n    store i32 %3 @out\n    ret void\n";
    let map = vec![
        ("a", 0x20),
        ("b", 0x24),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
        ("addm::x", 0x3C),
        ("addm::y", 0x40),
        ("addm::r", 0x44),
    ];
    let got = sim_run_bytes(
        ir,
        &str_map(&map),
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x05),
        ],
        0x28,
        4,
    );
    assert_eq!(
        &got[..],
        &[0x7D, 0x56, 0x34, 0x12],
        "addm(0x12345678, 5) must be 0x1234567D"
    );
}

// ---------------------------------------------------------------------------
// Milestone 12, Task 3: the i32 mul/div/rem/shift runtime routines.
// ---------------------------------------------------------------------------
//
// The 8 i32 routines (legalize-injected Funcs with 4-byte params + the
// scratch alloca per the layout contract) get recipe bodies in isel —
// panic-first on the names, then the recipes. SIM tests are the load-bearing
// checks: fixed inputs assembled + run in pic14_sim, result bytes asserted.

/// Build the module for an i32 routine: `main` loads two globals, calls the
/// routine, stores the result. Globals at 0x20-0x2B, main's locals at
/// 0x2C-0x37, the routine's params at 0x40-0x47, `__scr` at 0x48+ — all
/// ≤ 0x7F so the emitted asm assembles directly (bank 0, pre-banking).
fn routine_module32(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global ina {ret}\n\
         global inb {ret}\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load {ret} @ina\n\
             %y = load {ret} @inb\n\
             %r = call {ret} @{name}({ret} %x, {ret} %y)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let mut map = vec![
        ("ina".to_string(), 0x20),
        ("inb".to_string(), 0x24),
        ("out".to_string(), 0x28),
        ("main::x".to_string(), 0x2C),
        ("main::y".to_string(), 0x30),
        ("main::r".to_string(), 0x34),
    ];
    let mut base = 0x40u16;
    for (pn, _) in params {
        map.push((format!("{name}::{pn}"), base));
        base += 4;
    }
    map.push((format!("{name}::__scr"), base));
    (ir, map)
}

/// Every i32 routine emits a real recipe body — the label, the recipe
/// instructions, and a RETURN (never an empty label falling through into
/// the next function). `pats` are the load-bearing idiom strings at the
/// contract addresses (routine params at 0x40-0x47, `__scr` at 0x48+).
#[test]
fn i32_routines_emit_recipe_bodies() {
    let cases: &[(&str, &[&str])] = &[
        (
            "__mul_u32",
            &[
                "BTFSS 0x48, 0",  // bk_lo = __scr+0, multiplier bit test
                "INCFSZ 0x52, W", // t3 = __scr+10: the 32-bit carry idiom
                "ADDWF 0x4B, F",  // r0 = __scr+3
                "RLF 0x4F, F",    // t0 = __scr+7, tmp <<= 1
                "RRF 0x49, F",    // bk_hi = __scr+1, bk >>= 1
                "DECFSZ 0x4A, F", // cnt = __scr+2
            ],
        ),
        (
            "__udiv_u32",
            &[
                "RLF 0x40, F",    // num_lo <<= 1 (dividend param = quotient accumulator)
                "SUBWF 0x48, F",  // rem_lo = __scr+0
                "INCFSZ 0x4D, W", // den1 + borrow: the 4-byte borrow idiom
                "SUBWF 0x4B, F",  // rem_hi = __scr+3
                "BSF 0x40, 0",    // quotient bit into num
                "DECFSZ 0x50, F", // cnt = __scr+8, 32 iterations
            ],
        ),
        (
            "__urem_u32",
            &[
                "RLF 0x40, F",
                "SUBWF 0x48, F",
                "INCFSZ 0x4D, W",
                "SUBWF 0x4B, F",
                "BSF 0x40, 0", // the loop computes quotient + remainder
                "DECFSZ 0x50, F",
            ],
        ),
        (
            "__sdiv_i32",
            &[
                "BTFSS 0x43, 7", // num_hi sign test
                "COMF 0x40, F",  // |num| (32-bit) in place
                "BSF 0x52, 1",   // flags = __scr+10, bit1: remainder negate
                "BSF 0x52, 0",   // bit0: quotient negate
                "XORWF 0x52, F", // bit0 ^= den<0: neg_q = num<0 XOR den<0
                "BTFSS 0x52, 0", // tail: negate the quotient
            ],
        ),
        (
            "__srem_i32",
            &[
                "BTFSS 0x43, 7",
                "COMF 0x40, F",
                "BSF 0x52, 1",
                "BTFSS 0x52, 1", // tail: remainder sign follows dividend
                "COMF 0x48, F",  // rem_lo = __scr+0 negated
            ],
        ),
        (
            "__shl_u32",
            &["ANDLW 0x1F", "RLF 0x40, F", "DECFSZ 0x48, F"],
        ),
        (
            "__lshr_u32",
            &["ANDLW 0x1F", "RRF 0x43, F", "DECFSZ 0x48, F"],
        ),
        (
            "__ashr_i32",
            &[
                "ANDLW 0x1F",
                "BTFSC 0x43, 7",
                "RRF 0x43, F",
                "DECFSZ 0x48, F",
            ],
        ),
    ];
    for &(name, pats) in cases {
        let (ir, map) = routine_module32(name);
        let asm = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        assert!(
            asm.contains(&format!("    CALL {name}")),
            "{name} call:\n{asm}"
        );
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body
            .split("main:")
            .next()
            .expect("main label after routine");
        assert!(
            body.contains("    RETURN"),
            "{name} body must end in RETURN, not fall through:\n{asm}"
        );
        for p in pats {
            assert!(asm.contains(p), "{name} must contain `{p}`:\n{asm}");
        }
        assert!(
            body.contains("INCFSZ")
                || body.contains("RLF")
                || body.contains("COMF")
                || body.contains("ANDLW"),
            "{name} body looks like an empty label:\n{asm}"
        );
    }
}

/// The load-bearing i32 routine simulations: each routine's emitted asm is
/// assembled and run in pic14_sim with fixed inputs; the result bytes are
/// asserted. A wrong carry/borrow idiom, a wrong sign-wrapper step, or a
/// wrong shift mask flips a result.
#[test]
fn i32_routines_simulate_correctly() {
    // (routine, x bytes lo..hi, y bytes lo..hi, expected result bytes)
    let cases: &[(&str, &[u8], &[u8], &[u8])] = &[
        // 0x00010001 * 0x00010001 = 0x00020001 (bits 0 and 16 of b set).
        ("__mul_u32", &[1, 0, 1, 0], &[1, 0, 1, 0], &[1, 0, 2, 0]),
        // 0xFFFFFFFF * 2 = 0xFFFFFFFE: the shifted-out high bits are
        // DISCARDED (the 4-byte tmp wraps) — i32 mul wraps mod 2^32.
        (
            "__mul_u32",
            &[0xFF, 0xFF, 0xFF, 0xFF],
            &[2, 0, 0, 0],
            &[0xFE, 0xFF, 0xFF, 0xFF],
        ),
        // 0x12345678 / 0x100 = 0x123456 r 0x78 (the brief's "0x12345" was an
        // arithmetic slip: 0x12345678 = 0x123456*0x100 + 0x78).
        (
            "__udiv_u32",
            &[0x78, 0x56, 0x34, 0x12],
            &[0, 1, 0, 0],
            &[0x56, 0x34, 0x12, 0],
        ),
        (
            "__urem_u32",
            &[0x78, 0x56, 0x34, 0x12],
            &[0, 1, 0, 0],
            &[0x78, 0, 0, 0],
        ),
        // 0x12345678 / 0x1000 = 0x12345 r 0x678 — the brief's quotient figure,
        // with the divisor that actually produces it.
        (
            "__udiv_u32",
            &[0x78, 0x56, 0x34, 0x12],
            &[0, 0x10, 0, 0],
            &[0x45, 0x23, 1, 0],
        ),
        (
            "__urem_u32",
            &[0x78, 0x56, 0x34, 0x12],
            &[0, 0x10, 0, 0],
            &[0x78, 6, 0, 0],
        ),
        // -19 / 3 = -6 (neg_q = num<0 XOR den<0).
        (
            "__sdiv_i32",
            &[0xED, 0xFF, 0xFF, 0xFF],
            &[3, 0, 0, 0],
            &[0xFA, 0xFF, 0xFF, 0xFF],
        ),
        // 0x80000000 / -1 = 0x80000000: LLVM calls this poison, but the
        // routine's unsigned-abs path is deterministic — |INT_MIN| wraps to
        // itself, the abs'd den is 1, and the sign XOR cancels (num<0 and
        // den<0 both set bit0). Documented, deterministic, never a hang.
        (
            "__sdiv_i32",
            &[0, 0, 0, 0x80],
            &[0xFF, 0xFF, 0xFF, 0xFF],
            &[0, 0, 0, 0x80],
        ),
        // -19 % 3 = -1 (the remainder sign follows the dividend).
        (
            "__srem_i32",
            &[0xED, 0xFF, 0xFF, 0xFF],
            &[3, 0, 0, 0],
            &[0xFF, 0xFF, 0xFF, 0xFF],
        ),
        // 1 << 31 = 0x80000000; 1 << 33 = 2 (count masked to 31); 1 << 3 = 8.
        ("__shl_u32", &[1, 0, 0, 0], &[31, 0, 0, 0], &[0, 0, 0, 0x80]),
        ("__shl_u32", &[1, 0, 0, 0], &[33, 0, 0, 0], &[2, 0, 0, 0]),
        ("__shl_u32", &[1, 0, 0, 0], &[3, 0, 0, 0], &[8, 0, 0, 0]),
        // 0x12345678 >> 17 = 0x91A (count 17 needs no mask wrap).
        (
            "__lshr_u32",
            &[0x78, 0x56, 0x34, 0x12],
            &[17, 0, 0, 0],
            &[0x1A, 9, 0, 0],
        ),
        // 0x80000000 >> 4 = 0x08000000 (logical); ashr sign-fills = 0xF8000000.
        ("__lshr_u32", &[0, 0, 0, 0x80], &[4, 0, 0, 0], &[0, 0, 0, 8]),
        (
            "__ashr_i32",
            &[0, 0, 0, 0x80],
            &[4, 0, 0, 0],
            &[0, 0, 0, 0xF8],
        ),
    ];
    for &(name, x, y, want) in cases {
        let (ir, map) = routine_module32(name);
        let mut seed = Vec::new();
        for (i, b) in x.iter().enumerate() {
            seed.push((0x20 + i as u16, *b));
        }
        for (i, b) in y.iter().enumerate() {
            seed.push((0x24 + i as u16, *b));
        }
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, want.len());
        assert_eq!(&got[..], want, "{name}({x:?}, {y:?}) must be {want:?}");
    }
}

/// An i32 routine frame straddling banks must fail loudly (same
/// skip-sensitivity rule as the i8/i16 recipes).
#[test]
#[should_panic(expected = "straddle banks")]
fn panics_on_banked_i32_routine_slot() {
    let (ir, mut map) = routine_module32("__mul_u32");
    for (k, v) in map.iter_mut() {
        if k == "__mul_u32::__scr" {
            *v = 0xA0; // bank 1, straddling the bank-0 params
        }
    }
    let _ = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
}

// ---------------------------------------------------------------------------
// Milestone 13, Task 2: interrupt entry/prologue/epilogue + literal (SFR)
// pointers. The F877A's interrupt vector is word 0x0004 — the ISR's code
// starts there (no GOTO — a GOTO's target page would depend on the
// interrupted PCLATH, which is unknowable), so the ISR is placed FIRST, with
// a `.org 4` pad after the 2-word reset entry, and its `ret` becomes the
// restore epilogue + RETFIE. Literal (`inttoptr`) pointers are bank-mirrored
// SFRs: a direct MOVF/MOVWF with no FSR and no BANKSEL.

#[test]
fn isr_emits_vector_entry_prologue_epilogue() {
    let m = parse(
        "global in i8\nglobal out i8\n\
         fn isr(void) [isr] ()\n  block entry:\n    %v = load i8 @in\n    store i8 %v @out\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("isr::v", 0x25)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("    org 0x0004"),
        "the vector pad to word 4:\n{asm}"
    );
    assert!(
        asm.contains(
            "    org 0x0004\nisr:\n    MOVWF 0x75\n    SWAPF 0x75, F\n    SWAPF STATUS, W\n    MOVWF 0x76\n    MOVF PCLATH, W\n    MOVWF 0x77\n    MOVF FSR, W\n    MOVWF 0x78\n    MOVF 0x71, W\n    MOVWF 0x79\n    MOVF 0x72, W\n    MOVWF 0x7A\n    MOVF 0x73, W\n    MOVWF 0x7B\n    MOVF 0x74, W\n    MOVWF 0x7C\n    MOVF 0x70, W\n    MOVWF 0x7D\n    MOVLW 0x00\n    MOVWF PCLATH"
        ),
        "the 20-line save prologue (W/STATUS/PCLATH/FSR/retval x4/scratch), right after the vector pad:\n{asm}"
    );
    assert!(
        asm.contains(
            "    MOVF 0x79, W\n    MOVWF 0x71\n    MOVF 0x7A, W\n    MOVWF 0x72\n    MOVF 0x7B, W\n    MOVWF 0x73\n    MOVF 0x7C, W\n    MOVWF 0x74\n    MOVF 0x7D, W\n    MOVWF 0x70\n    MOVF 0x77, W\n    MOVWF PCLATH\n    MOVF 0x78, W\n    MOVWF FSR\n    SWAPF 0x76, W\n    MOVWF STATUS\n    SWAPF 0x75, W\n    RETFIE"
        ),
        "the restore epilogue (retval x4, scratch, PCLATH/FSR, STATUS, W) + RETFIE:\n{asm}"
    );
    // The ISR is placed FIRST: its vector pad precedes main's label, and
    // __start moves after the ISR.
    let org = asm.find("    org 0x0004").unwrap();
    let main = asm.find("main:").unwrap();
    let start = asm.find("__start:").unwrap();
    assert!(
        org < start && start < main,
        "ISR first, then __start, then main:\n{asm}"
    );
    // Non-ISR functions keep the plain RETURN terminator.
    assert!(
        asm.contains("    RETURN"),
        "main's ret is unchanged:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "does not fit page 0")]
fn panics_on_isr_larger_than_page_0() {
    // The ISR is pinned at the vector (word 4); a body that cannot fit page
    // 0 (0x004-0x7FF) with room for the reset __start can never be padded
    // (the vector IS the entry), so it must panic loudly.
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn isr(void) [isr] ()\n  block entry:\n    %a = load i8 @in\n{}    store i8 %a @out\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
        pad_body(700)
    ));
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("isr::a", 0x25)]);
    let _ = select(&PIC16F877A, &m, &addrs);
}

#[test]
fn literal_ptr_store_emits_direct_movwf() {
    // `store i8 %v 0x06` — an inttoptr (SFR) pointer: the register is
    // bank-mirrored, so the access is a direct MOVWF with no FSR setup (and
    // isel emits no BANKSEL anywhere — the banking pass has nothing to add).
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    %v = load i8 @in\n    store i8 %v 0x06\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("main::v", 0x25)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(asm.contains("    MOVWF 0x06"), "direct SFR store:\n{asm}");
    assert!(
        !asm.contains("MOVWF FSR"),
        "no FSR for a literal (SFR) store:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF INDF"),
        "no INDF for a literal (SFR) store:\n{asm}"
    );
}

#[test]
fn literal_ptr_load_emits_direct_movf() {
    // `%v = load i8 0x06` — direct MOVF from the SFR, no FSR setup.
    let m = parse("global out i8\nfn main(void) ()\n  block entry:\n    %v = load i8 0x06\n    store i8 %v @out\n    ret void\n");
    let addrs = addrs(&[("out", 0x21), ("main::v", 0x25)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(asm.contains("    MOVF 0x06, W"), "direct SFR load:\n{asm}");
    assert!(
        !asm.contains("MOVWF FSR"),
        "no FSR for a literal (SFR) load:\n{asm}"
    );
    assert!(
        !asm.contains("MOVF INDF, W"),
        "no INDF for a literal (SFR) load:\n{asm}"
    );
}

#[test]
fn runtime_inttoptr_derefs_through_fsr_indf() {
    // epic-cc#117 shape 1: a standalone runtime `inttoptr` (the table-free
    // computed address, `read_offset`). The address bytes land in the dst
    // slot (0x28/0x29); the load lowers through FSR/INDF with IRP set from
    // the stored high byte, and NO BANKSEL anywhere: INDF reaches the whole
    // linear file space through FSR+IRP.
    let m = parse(
        "global off i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %o = load i8 @off\n    %a = inttoptr i16 %o to i16\n    %v = load i8 %a\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("off", 0x20),
        ("out", 0x21),
        ("main::o", 0x25),
        ("main::a", 0x26),
        ("main::v", 0x27),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The address bytes are copied from the source slot into the address
    // slot, then FSR = slot bytes and the access goes through INDF.
    assert!(
        asm.contains("MOVWF FSR") && asm.contains("MOVF INDF, W"),
        "indirect FSR/INDF access:\n{asm}"
    );
    assert!(
        !asm.contains("BANKSEL"),
        "no BANKSEL for an indirect SFR access:\n{asm}"
    );
}

#[test]
fn runtime_ptr_select_materializes_arm_then_derefs_indirect() {
    // A pointer select over two runtime address literals (the HAL's
    // `pir_is_pir2 ? PIR2 : PIR1` shape): the select writes the chosen
    // address's two bytes into the dst slot, and the deref goes through
    // FSR/INDF from the slot.
    let m = parse("global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %c = load i8 @c\n    %p = select i1 %c ptr 12 ptr 13\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n");
    let addrs = addrs(&[
        ("c", 0x20),
        ("out", 0x21),
        ("main::c", 0x25),
        ("main::p", 0x26),
        ("main::v", 0x27),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVLW 0x0C") && asm.contains("MOVLW 0x0D"),
        "both literal arms materialized:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF FSR") && asm.contains("MOVF INDF, W"),
        "indirect FSR/INDF access:\n{asm}"
    );
    assert!(
        !asm.contains("BANKSEL"),
        "no BANKSEL for an indirect SFR access:\n{asm}"
    );
}

#[test]
fn runtime_ptr_phi_derefs_through_slot_after_phi_copies() {
    // The GetFlag -O1 shape: a pointer phi joining a literal-arm select
    // result and the INTCON literal. Phi elimination copies the incoming's
    // two bytes into the dst slot per edge; the deref goes indirect from
    // the slot.
    let m = parse(
        "global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %c = load i8 @c\n    br i1 %c t f\n  block t:\n    %pt = select i1 %c ptr 12 ptr 13\n    br merge\n  block f:\n    br merge\n  block merge:\n    %p = phi ptr %pt t 11 f\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("c", 0x20),
        ("out", 0x21),
        ("main::c", 0x25),
        ("main::pt", 0x26),
        ("main::p", 0x28),
        ("main::v", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("MOVWF FSR") && asm.contains("MOVF INDF, W"),
        "indirect FSR/INDF access:\n{asm}"
    );
    assert!(
        !asm.contains("BANKSEL"),
        "no BANKSEL for an indirect SFR access:\n{asm}"
    );
}

#[test]
fn banking_selects_bank0_for_sfr_and_leaves_save_area_untouched() {
    // The common GPR block (0x70-0x7F) and the mirrored core registers need
    // no banking; a non-mirrored bank-0 SFR (PORTB 0x06) is reachable only
    // with RP1:RP0 = 0, so the pass selects bank 0 before it when the bank
    // is unknown or differs. The ISR save area must pass through
    // `assign_banks` unchanged: no BANKSEL inserted for it, no operand
    // rewritten. (Bank-0 body operands get a full BANKSEL after each label
    // too: the interrupted program's bank is unknown at an ISR entry, and
    // the SFR store rides on the body's select.)
    let m = parse(
        "global in i8\nglobal out i8\n\
         fn isr(void) [isr] ()\n  block entry:\n    %v = load i8 @in\n    store i8 %v 0x06\n    ret void\n\
         fn main(void) ()\n  block entry:\n    %m = load i8 0x06\n    store i8 %m @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("isr::v", 0x25),
        ("main::m", 0x26),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    let banked = banking::assign_banks(&device::PIC16F877A, &asm);
    // The SFR store follows the value load directly: the body's banked
    // operand already selected bank 0, so no BANKSEL is inserted between
    // the load and the store. epic-cc#214: %v's own reload for the store
    // is also redundant (still in W right after Inst::Load's own store to
    // 0x25) and gets elided, leaving just the SFR write itself.
    assert!(
        banked.contains("    MOVWF 0x25\n    MOVWF 0x06"),
        "SFR store is direct with no BANKSEL:\n{banked}"
    );
    // The SFR load is the first instruction after main's label: the bank
    // is unknown there, so the bank-0 SFR gets a full bank-0 select (issue
    // #112: without it the load would read the bank-1 SFR at 0x86).
    assert!(
        banked.contains("main:\n    BCF STATUS, 5\n    BCF STATUS, 6\n    MOVF 0x06, W"),
        "SFR load gets a bank-0 select at the label:\n{banked}"
    );
    // The save area (common RAM 0x75-0x7D, scratch included) passes
    // through untouched.
    assert!(
        banked.contains("    MOVWF 0x75\n    SWAPF 0x75, F\n    SWAPF STATUS, W\n    MOVWF 0x76\n    MOVF PCLATH, W\n    MOVWF 0x77\n    MOVF FSR, W\n    MOVWF 0x78\n    MOVF 0x71, W\n    MOVWF 0x79\n    MOVF 0x72, W\n    MOVWF 0x7A\n    MOVF 0x73, W\n    MOVWF 0x7B\n    MOVF 0x74, W\n    MOVWF 0x7C\n    MOVF 0x70, W\n    MOVWF 0x7D"),
        "the ISR save area survives banking untouched:\n{banked}"
    );
}

#[test]
fn isr_prologue_body_epilogue_simulates_with_retfie_return() {
    // The full interrupt path (vector fire -> PC = 4) needs the sim's
    // fire_interrupt hook (milestone 13, task 3); until then the test
    // reaches the vector the way the hardware would leave it: the main
    // context CALLs word 4, so the emitted prologue/body/epilogue + RETFIE
    // all execute, and RETFIE returns to the exact instruction after the
    // CALL — the interrupted computation completes with the correct result.
    //
    // The internal asm crate does not yet encode SWAPF/RETFIE, and task 2
    // must not touch crates/asm (gpasm covers the real assembly in task 5),
    // so this test hand-encodes the exact words isel emits (the emitted
    // TEXT is asserted verbatim by isr_emits_vector_entry_prologue_epilogue
    // and banking_leaves_sfr_and_isr_save_area_untouched; every word below
    // is the encoding of the corresponding emitted instruction).
    //
    // Layout: word 0 = reset GOTO __start (word 60); words 1-3 = the
    // `.org 4` pad; words 4-55 = the ISR (save prologue 4-23, body 24-37,
    // restore epilogue 38-54, RETFIE 55); words 56-59 = the same-page
    // helper (returns through the retval region: 0x71 = in + 1); words
    // 60-71 = __start (the "main" context).
    //
    // The interrupted context is built in __start: W = 0x41 (0x42 +
    // 0xFF), STATUS = 0x03 (C+DC from the ADDLW), PCLATH = 0, FSR = 0x12
    // (pre-seeded), an in-flight return value 0x11/0x22/0x33/0x44 in the
    // retval region 0x71-0x74, and an in-flight scratch byte 0x5A in 0x70
    // (all pre-seeded — main was "inside" a call whose result it had not
    // yet consumed and mid-icmp with a value folded through the scratch).
    // The ISR body writes in -> isr_g (0x21), calls the helper (in+1 ->
    // hlp_g 0x22, 0x71), then clobbers W/STATUS/FSR/PCLATH, the retval
    // bytes 0x72-0x74, and the scratch byte 0x70 (an ISR that uses the
    // scratch); the epilogue must restore every one of them from
    // 0x75-0x7D.
    use pic14_sim::Pic14;
    let words: Vec<u16> = vec![
        0x283C, //  0: GOTO 60 (__start)
        0x0000, //  1: .org 4 pad
        0x0000, //  2
        0x0000, //  3
        0x00F5, //  4: MOVWF 0x75        save W
        0x0EF5, //  5: SWAPF 0x75, F     swap W in place (no STATUS side effects)
        0x0E03, //  6: SWAPF STATUS, W   STATUS -> W without touching it
        0x00F6, //  7: MOVWF 0x76        save SWAPF(STATUS)
        0x080A, //  8: MOVF PCLATH, W
        0x00F7, //  9: MOVWF 0x77        save PCLATH
        0x0804, // 10: MOVF FSR, W
        0x00F8, // 11: MOVWF 0x78        save FSR
        0x0871, // 12: MOVF 0x71, W      save the in-flight retval bytes
        0x00F9, // 13: MOVWF 0x79
        0x0872, // 14: MOVF 0x72, W
        0x00FA, // 15: MOVWF 0x7A
        0x0873, // 16: MOVF 0x73, W
        0x00FB, // 17: MOVWF 0x7B
        0x0874, // 18: MOVF 0x74, W
        0x00FC, // 19: MOVWF 0x7C
        0x0870, // 20: MOVF 0x70, W      save the in-flight scratch byte
        0x00FD, // 21: MOVWF 0x7D
        0x3000, // 22: MOVLW 0x00
        0x008A, // 23: MOVWF PCLATH     ISR body runs in page 0
        0x0820, // 24: MOVF 0x20, W      body: W = in
        0x00A1, // 25: MOVWF 0x21        isr_g = in (the ISR's global write)
        0x00AA, // 26: MOVWF 0x2A        helper's param slot = in
        0x3000, // 27: MOVLW 0x00        PAGE(helper)
        0x008A, // 28: MOVWF PCLATH
        0x2038, // 29: CALL 56           same-page helper
        0x00A2, // 30: MOVWF 0x22        hlp_g = helper(in) = in + 1
        0x30FF, // 31: MOVLW 0xFF        clobber W
        0x0084, // 32: MOVWF FSR         clobber FSR
        0x008A, // 33: MOVWF PCLATH      clobber PCLATH
        0x00F2, // 34: MOVWF 0x72        clobber retval byte 1
        0x00F3, // 35: MOVWF 0x73        clobber retval byte 2
        0x00F4, // 36: MOVWF 0x74        clobber retval byte 3
        0x00F0, // 37: MOVWF 0x70        clobber the scratch (the ISR uses it)
        0x0879, // 38: MOVF 0x79, W      epilogue: retval first (MOVF Z
        0x00F1, // 39: MOVWF 0x71        clobbers are fine: STATUS not yet
        0x087A, // 40: MOVF 0x7A, W      restored)
        0x00F2, // 41: MOVWF 0x72
        0x087B, // 42: MOVF 0x7B, W
        0x00F3, // 43: MOVWF 0x73
        0x087C, // 44: MOVF 0x7C, W
        0x00F4, // 45: MOVWF 0x74
        0x087D, // 46: MOVF 0x7D, W      then the scratch (Z clobber fine —
        0x00F0, // 47: MOVWF 0x70        STATUS is not yet restored)
        0x0877, // 48: MOVF 0x77, W      W = saved PCLATH
        0x008A, // 49: MOVWF PCLATH      restore PCLATH
        0x0878, // 50: MOVF 0x78, W      W = saved FSR
        0x0084, // 51: MOVWF FSR         restore FSR
        0x0E76, // 52: SWAPF 0x76, W     W = swap(saved STATUS)
        0x0083, // 53: MOVWF STATUS      restore STATUS (flag-safe)
        0x0E75, // 54: SWAPF 0x75, W     W = saved W (swap-back, flag-safe — last)
        0x0009, // 55: RETFIE
        0x082A, // 56: helper: MOVF 0x2A, W
        0x3E01, // 57: ADDLW 0x01        W = in + 1
        0x00F1, // 58: MOVWF 0x71        result through the retval region
        0x0008, // 59: RETURN
        0x3042, // 60: __start: MOVLW 0x42
        0x3EFF, // 61: ADDLW 0xFF        W = 0x41, STATUS = 0x03 (interrupted ctx)
        0x2004, // 62: CALL 4            "interrupt" — push 63, jump to vector
        0x00A3, // 63: MOVWF 0x23        out_w = restored W (0x41)
        0x080A, // 64: MOVF PCLATH, W
        0x00A4, // 65: MOVWF 0x24        out_pclath = restored PCLATH (0)
        0x0804, // 66: MOVF FSR, W
        0x00A5, // 67: MOVWF 0x25        out_fsr = restored FSR (0x12)
        0x0803, // 68: MOVF STATUS, W
        0x00A6, // 69: MOVWF 0x26        out_status = restored STATUS (0x03)
        0x2847, // 70: GOTO 71           needs the restored PCLATH = 0
        0x0063, // 71: SLEEP
    ];
    let mut p = Pic14::new(words);
    p.ram_mut()[0x20] = 0x42; // in
    p.ram_mut()[0x04] = 0x12; // the interrupted context's FSR
    p.ram_mut()[0x70] = 0x5A; // the interrupted context's in-flight scratch
    p.ram_mut()[0x71] = 0x11; // the interrupted context's in-flight retval
    p.ram_mut()[0x72] = 0x22;
    p.ram_mut()[0x73] = 0x33;
    p.ram_mut()[0x74] = 0x44;
    p.run(1000);
    assert!(p.halted(), "program must SLEEP-halt");
    // The ISR body's writes: its own global and the same-page helper result.
    assert_eq!(p.ram()[0x21], 0x42, "isr_g = in (the ISR ran)");
    assert_eq!(
        p.ram()[0x22],
        0x43,
        "hlp_g = helper(in) = in + 1 (same-page call from the ISR)"
    );
    // The interrupted computation completes: every restored register lands.
    assert_eq!(
        p.ram()[0x23],
        0x41,
        "out_w = restored W (body left W = 0xFF)"
    );
    assert_eq!(
        p.ram()[0x24],
        0x00,
        "out_pclath = restored PCLATH (body left 0xFF)"
    );
    assert_eq!(
        p.ram()[0x25],
        0x12,
        "out_fsr = restored FSR (body left 0xFF)"
    );
    assert_eq!(
        p.ram()[0x26],
        0x03,
        "out_status = restored STATUS (body left 0x00)"
    );
    // The in-flight retval survives the ISR: the helper wrote 0x71 = 0x43
    // and the body wrote 0xFF into 0x72-0x74, but the epilogue restored
    // main's 0x11/0x22/0x33/0x44 from the extended save area.
    assert_eq!(
        p.ram()[0x71],
        0x11,
        "restored retval byte 0 (helper wrote 0x43)"
    );
    assert_eq!(
        p.ram()[0x72],
        0x22,
        "restored retval byte 1 (body wrote 0xFF)"
    );
    assert_eq!(
        p.ram()[0x73],
        0x33,
        "restored retval byte 2 (body wrote 0xFF)"
    );
    assert_eq!(
        p.ram()[0x74],
        0x44,
        "restored retval byte 3 (body wrote 0xFF)"
    );
    // The in-flight scratch survives the ISR: the body clobbered 0x70 with
    // 0xFF, but the epilogue restored main's 0x5A from the 9-byte save
    // area.
    assert_eq!(p.ram()[0x70], 0x5A, "restored scratch (body wrote 0xFF)");
    // The save area (fixed common RAM 0x75-0x7D, disjoint from scratch
    // 0x70 and the retval region 0x71-0x74; 0x7E-0x7F stays free):
    // SWAPF(W), SWAPF(STATUS), PCLATH, FSR, retval x4, scratch at vector
    // entry.
    assert_eq!(p.ram()[0x75], 0x14, "saved W nibble-swapped (0x41 -> 0x14)");
    assert_eq!(
        p.ram()[0x76],
        0x30,
        "saved STATUS nibble-swapped (0x03 -> 0x30)"
    );
    assert_eq!(p.ram()[0x77], 0x00, "saved PCLATH");
    assert_eq!(p.ram()[0x78], 0x12, "saved FSR");
    assert_eq!(p.ram()[0x79], 0x11, "saved retval byte 0");
    assert_eq!(p.ram()[0x7A], 0x22, "saved retval byte 1");
    assert_eq!(p.ram()[0x7B], 0x33, "saved retval byte 2");
    assert_eq!(p.ram()[0x7C], 0x44, "saved retval byte 3");
    assert_eq!(p.ram()[0x7D], 0x5A, "saved scratch");
}

#[test]
fn isr_epilogue_preserves_preempted_z_for_main_branch() {
    // The M13-T5 regression: the OLD epilogue restored STATUS (flag-safe
    // SWAPF) but then restored FSR and W with MOVF — which SETS Z from the
    // moved value AFTER STATUS was already restored, corrupting the
    // interrupted main's Z. Here main sets Z = 1 (`ADDWF 0x20, F` -> 0x00)
    // with W = 0xFF non-zero, the ISR fires between the Z-setting ADDWF
    // and the Z-consuming BTFSS, the ISR body leaves
    // W/STATUS/FSR/PCLATH clobbered (W = 0x5A, Z = 0, FSR = 0x00,
    // PCLATH = 0x18), and main's branch must still take the preempted
    // Z = 1 path (out = 0xAA). Pre-fix, the epilogue's final MOVF 0x75, W
    // clears Z (saved W = 0xFF != 0), so main falls into the wrong branch
    // (out = 0xBB).
    //
    // fire_interrupt pushes pc+1 and jumps to the vector: firing at the
    // NOP at word 55 (between the ADDWF at 54 and the BTFSS at 56) resumes
    // main at 56, so the Z test runs against the restored STATUS.
    //
    // Words are the exact encodings of the emitted prologue/epilogue (the
    // TEXT is asserted verbatim by isr_emits_vector_entry_prologue_epilogue).
    use pic14_sim::Pic14;
    let words: Vec<u16> = vec![
        0x2831, //  0: GOTO 49 (__start)
        0x0000, //  1: .org 4 pad
        0x0000, //  2
        0x0000, //  3
        0x00F5, //  4: MOVWF 0x75        prologue: save W
        0x0EF5, //  5: SWAPF 0x75, F     swap W in place (flag-safe)
        0x0E03, //  6: SWAPF STATUS, W
        0x00F6, //  7: MOVWF 0x76        save SWAPF(STATUS)
        0x080A, //  8: MOVF PCLATH, W
        0x00F7, //  9: MOVWF 0x77        save PCLATH
        0x0804, // 10: MOVF FSR, W
        0x00F8, // 11: MOVWF 0x78        save FSR
        0x0871, // 12: MOVF 0x71, W      save the in-flight retval bytes
        0x00F9, // 13: MOVWF 0x79
        0x0872, // 14: MOVF 0x72, W
        0x00FA, // 15: MOVWF 0x7A
        0x0873, // 16: MOVF 0x73, W
        0x00FB, // 17: MOVWF 0x7B
        0x0874, // 18: MOVF 0x74, W
        0x00FC, // 19: MOVWF 0x7C
        0x0870, // 20: MOVF 0x70, W      save the scratch byte
        0x00FD, // 21: MOVWF 0x7D
        0x3000, // 22: MOVLW 0x00
        0x008A, // 23: MOVWF PCLATH     ISR body runs in page 0
        0x305A, // 24: MOVLW 0x5A        body: clobber W (non-zero)
        0x00C0, // 25: MOVWF 0x40        isr_g = 0x5A (the ISR ran)
        0x0850, // 26: MOVF 0x50, W      clobber Z (RAM[0x50] = 0x5A -> Z = 0)
        0x3000, // 27: MOVLW 0x00
        0x0084, // 28: MOVWF FSR         clobber FSR
        0x3018, // 29: MOVLW 0x18
        0x008A, // 30: MOVWF PCLATH     clobber PCLATH
        0x0879, // 31: MOVF 0x79, W      epilogue: retval first (MOVF Z
        0x00F1, // 32: MOVWF 0x71        clobbers are fine: STATUS not yet
        0x087A, // 33: MOVF 0x7A, W      restored)
        0x00F2, // 34: MOVWF 0x72
        0x087B, // 35: MOVF 0x7B, W
        0x00F3, // 36: MOVWF 0x73
        0x087C, // 37: MOVF 0x7C, W
        0x00F4, // 38: MOVWF 0x74
        0x087D, // 39: MOVF 0x7D, W      then the scratch (Z clobber fine —
        0x00F0, // 40: MOVWF 0x70        STATUS is not yet restored)
        0x0877, // 41: MOVF 0x77, W      then PCLATH and FSR
        0x008A, // 42: MOVWF PCLATH
        0x0878, // 43: MOVF 0x78, W
        0x0084, // 44: MOVWF FSR
        0x0E76, // 45: SWAPF 0x76, W     then STATUS (flag-safe)
        0x0083, // 46: MOVWF STATUS
        0x0E75, // 47: SWAPF 0x75, W     W last (swap-back, flag-safe)
        0x0009, // 48: RETFIE
        0x3012, // 49: __start: MOVLW 0x12
        0x0084, // 50: MOVWF FSR         interrupted ctx FSR = 0x12
        0x3001, // 51: MOVLW 0x01
        0x00A0, // 52: MOVWF 0x20        RAM[0x20] = 1
        0x30FF, // 53: MOVLW 0xFF
        0x07A0, // 54: ADDWF 0x20, F     Z = 1 (0x01 + 0xFF = 0x00), C/DC set; W = 0xFF
        0x0000, // 55: NOP               <- fire here: push 56, jump to the vector
        0x1D03, // 56: BTFSS STATUS, 2   the Z-consuming instruction
        0x283D, // 57: GOTO 61 (wrong)   taken only when Z == 0
        0x30AA, // 58: MOVLW 0xAA
        0x00B0, // 59: MOVWF 0x30        out = 0xAA (the preempted Z = 1 path)
        0x283F, // 60: GOTO 63 (done)
        0x30BB, // 61: wrong: MOVLW 0xBB
        0x00B0, // 62: MOVWF 0x30        out = 0xBB
        0x0804, // 63: done: MOVF FSR, W
        0x00B1, // 64: MOVWF 0x31        out_fsr = restored FSR (0x12)
        0x080A, // 65: MOVF PCLATH, W
        0x00B2, // 66: MOVWF 0x32        out_pclath = restored PCLATH (0)
        0x0063, // 67: SLEEP
    ];
    let mut p = Pic14::new(words);
    p.ram_mut()[0x50] = 0x5A; // the ISR body's MOVF source: W = 0x5A, Z = 0
                              // Run main up to the Z test: the ADDWF at word 54 just set Z = 1 with
                              // W = 0xFF (non-zero, pre-fix the epilogue's MOVF 0x75, W clears Z
                              // from exactly this saved W).
    let mut steps = 0usize;
    while p.pc() != 55 {
        p.step();
        steps += 1;
        assert!(steps < 100, "never reached the NOP (pc = {})", p.pc());
    }
    assert_eq!(p.ram()[0x03] & 0x04, 0x04, "the ADDWF must have set Z = 1");
    assert_eq!(
        p.w(),
        0xFF,
        "W must be non-zero at the fire (the pre-fix MOVF restore clears Z from it)"
    );
    p.fire_interrupt();
    assert_eq!(p.pc(), 4, "the ISR starts at the vector");
    p.run(500_000);
    assert!(p.halted(), "program must SLEEP-halt");
    assert_eq!(p.ram()[0x40], 0x5A, "isr_g = 0x5A (the ISR ran)");
    assert_eq!(
        p.ram()[0x30],
        0xAA,
        "main's branch must take the preempted Z = 1 path (a Z-clobbering epilogue lands 0xBB)"
    );
    assert_eq!(
        p.ram()[0x31],
        0x12,
        "out_fsr = restored FSR (body left 0x00)"
    );
    assert_eq!(
        p.ram()[0x32],
        0x00,
        "out_pclath = restored PCLATH (body left 0x18)"
    );
}

/// Walk the asm counting words (the same rules as `label_addr`), returning
/// the address of the first line whose trimmed text is `needle` after the
/// `label:` line.
fn line_addr_after(asm: &str, label: &str, needle: &str) -> usize {
    let mut org = 0usize;
    let mut in_label = false;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        if let Some(l) = line.strip_suffix(':') {
            in_label = l.trim() == label;
            continue;
        }
        if line.contains(" equ ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        if line.starts_with(".table ") {
            continue;
        }
        if in_label && line == needle {
            return org;
        }
        org += 1;
    }
    panic!("{needle} after {label} not found");
}

/// M13-final SIM regression: the fixed scratch byte (0x70) is LIVE across
/// interrupt windows — the i16/i32 icmp chains fold through it, const
/// reads stash their byte/index in it across the PCLATH restore, GEP
/// offsets accumulate there — so an ISR that itself uses the scratch (its
/// own icmp writes 0x70) must not corrupt the preempted main's in-flight
/// value. main folds an i16 equality through the scratch; the interrupt
/// fires right after main's stash lands (while the value is still live),
/// and the ISR's i8 equality clobbers the scratch. The prologue must save
/// 0x70 -> 0x7D and the epilogue restore it before main resumes — pre-fix
/// (no save) the IORWF fold reads the ISR's 0x00 and main reports the
/// wrong equality.
///
/// Value shape (load-bearing): a = 0x1212, b = 0x1200 with a0^b0 = 0x12 ==
/// a1, so the a1-load the ISR preempts is re-derivable from the restored W
/// (the fold computes the same XOR either way) — the ONLY difference
/// between pre-fix and post-fix is the fate of the live 0x12 in the
/// scratch. The ISR compares c == d (equal -> XOR 0x00): the restored
/// 0x12 | 0x00 = 0x12 leaves Z clear (not equal -> out 0), the clobbered
/// 0x00 | 0x00 = 0x00 sets Z (equal -> out 1).
#[test]
fn isr_scratch_use_does_not_corrupt_preempted_main() {
    let m = parse(
        "global a i16\nglobal b i16\nglobal out i8\nglobal c i8\nglobal d i8\nglobal isr_g i8\n\
         fn main(void) ()\n  block entry:\n    %av = load i16 @a\n    %bv = load i16 @b\n    %e = icmp eq i16 %av, %bv\n    store i8 %e @out\n    ret void\n\
         fn isr(void) [isr] ()\n  block entry:\n    %cv = load i8 @c\n    %dv = load i8 @d\n    %eq = icmp eq i8 %cv, %dv\n    store i8 %eq @isr_g\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x22),
        ("out", 0x24),
        ("c", 0x25),
        ("d", 0x26),
        ("isr_g", 0x27),
        ("main::av", 0x30),
        ("main::bv", 0x32),
        ("main::e", 0x34),
        ("isr::cv", 0x35),
        ("isr::dv", 0x36),
        ("isr::eq", 0x37),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // main's i16 eq folds through the scratch: the low-byte XOR is stashed
    // in 0x70 and ORed with the high-byte XOR on the way out.
    assert!(
        asm.contains("    MOVWF 0x70\n    MOVF 0x31, W\n    XORWF 0x33, W\n    IORWF 0x70, W"),
        "main's i16 eq must fold through the scratch:\n{asm}"
    );
    let stash = line_addr_after(&asm, "main", "MOVWF 0x70");
    use pic14_sim::Pic14;
    let words = asm::assemble(&asm);
    let mut p = Pic14::new(words);
    p.ram_mut()[0x20] = 0x12; // a lo
    p.ram_mut()[0x21] = 0x12; // a hi
    p.ram_mut()[0x22] = 0x00; // b lo
    p.ram_mut()[0x23] = 0x12; // b hi
    p.ram_mut()[0x25] = 0x00; // c
    p.ram_mut()[0x26] = 0x00; // d
                              // Run to the stash, step it (0x70 = 0x12, main's in-flight value), then
                              // fire the interrupt while the value is live in the scratch.
    let mut steps = 0usize;
    while p.pc() != stash as u16 {
        p.step();
        steps += 1;
        assert!(steps < 100, "never reached the stash (pc = {})", p.pc());
    }
    p.step(); // MOVWF 0x70: the in-flight 0x12 lands in the scratch
    assert_eq!(
        p.ram()[0x70],
        0x12,
        "main's in-flight value is live in the scratch"
    );
    p.fire_interrupt();
    assert_eq!(p.pc(), 4, "the ISR starts at the vector");
    p.run(500_000);
    assert!(p.halted(), "program must SLEEP-halt");
    // The ISR ran and clobbered the scratch with its own equality result
    // (0x00: c == d, equal -> isr_g 1); the epilogue must have restored
    // main's 0x12 before the IORWF fold.
    assert_eq!(
        p.ram()[0x27],
        0x01,
        "isr_g = the ISR's equality (c == d -> 1)"
    );
    assert_eq!(
        p.ram()[0x24],
        0x00,
        "main's i16 eq must still report 0x1212 != 0x1200 (pre-fix the ISR's scratch clobber flips it to 1)"
    );
}

// ---------------------------------------------------------------------------
// Milestone 15, Task 3: the soft-float runtime routines (isel recipes).
// ---------------------------------------------------------------------------
//
// The routine Funcs are injected by legalize with i32 params (4-byte slots —
// the f32-ness rides on the call types, per the Task-2 contract), one
// `%__scr = alloca N` entry block, and NO `ret` — isel emits the recipe body
// plus the RETURN. The float format: 4 bytes LE: b0 = mantissa LSB, b1, b2 =
// mantissa MSB + the exponent's LSB (bit 7 of b2), b3 = sign | exponent[7:1];
// the 24-bit mantissa = (b2 & 0x7F) << 16 | b1 << 8 | b0, plus the implicit
// 0x800000 when the 8-bit biased exponent ((b3 & 0x7F) << 1 | (b2 >> 7)) is
// nonzero. All slots stay ≤ 0x7F (bank 0, loud) — the loops are
// skip-sensitive.

/// The injected routine signatures (ret, params, `__scr` size) for the nine
/// float routines, mirroring legalize's injection exactly (the Task-2
/// contract). Params are i32 (4-byte slots) regardless of the f32-ness.
fn float_routine_sig(name: &str) -> (&'static str, &'static [(&'static str, &'static str)], u16) {
    match name {
        "__add_f32" | "__sub_f32" | "__mul_f32" => ("float", &[("a", "i32"), ("b", "i32")], 14),
        "__div_f32" => ("float", &[("a", "i32"), ("b", "i32")], 12),
        "__cmp_f32" => ("i8", &[("a", "i32"), ("b", "i32")], 6),
        "__uitofp_f32" | "__sitofp_f32" => ("float", &[("val", "i32")], 8),
        "__fptoui_f32" | "__fptosi_f32" => ("i32", &[("val", "i32")], 8),
        other => panic!("test: unknown routine {other}"),
    }
}

/// The binary float routine module: `main` loads two f32 globals, calls the
/// routine (the injected Func def written out exactly as legalize produces
/// it), stores the result. Globals at 0x20/0x24/0x28, main's locals at
/// 0x2C/0x30/0x34, the routine's params at 0x40/0x44, `__scr` at 0x48+ — all
/// ≤ 0x7F so the raw emitted asm assembles directly (bank 0, pre-banking).
fn float_routine_module(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = float_routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global ina float\n\
         global inb float\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load float @ina\n\
             %y = load float @inb\n\
             %r = call {ret} @{name}(float %x, float %y)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let mut map = vec![
        ("ina".to_string(), 0x20u16),
        ("inb".to_string(), 0x24),
        ("out".to_string(), 0x28),
        ("main::x".to_string(), 0x2C),
        ("main::y".to_string(), 0x30),
        ("main::r".to_string(), 0x34),
    ];
    let mut base = 0x40u16;
    for (pn, _) in params {
        map.push((format!("{name}::{pn}"), base));
        base += 4;
    }
    map.push((format!("{name}::__scr"), base));
    (ir, map)
}

/// The unary float routine module: `main` loads one 4-byte global, calls the
/// routine, stores the result. inv=0x20, out=0x24, main::v=0x2C, main::r=0x30,
/// the routine's val param at 0x40, `__scr` at 0x44+ — all ≤ 0x7F.
fn float_routine_unary_module(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = float_routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global inv i32\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i32 @inv\n\
             %r = call {ret} @{name}(i32 %v)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let map = vec![
        ("inv".to_string(), 0x20u16),
        ("out".to_string(), 0x24),
        ("main::v".to_string(), 0x2C),
        ("main::r".to_string(), 0x30),
        (format!("{name}::val"), 0x40),
        (format!("{name}::__scr"), 0x44),
    ];
    (ir, map)
}

/// The f32 bit pattern of a Rust f32, little-endian (the routine's byte
/// order: b0 = mantissa LSB ... b3 = sign|exp).
fn f32_le(x: f32) -> [u8; 4] {
    x.to_bits().to_le_bytes()
}

/// The load-bearing soft-float arithmetic simulations: each routine's emitted
/// asm is assembled and run in pic14_sim with fixed inputs; the 4-byte result
/// is asserted bit-for-bit against the value computed by Rust's OWN f32
/// arithmetic (round-to-nearest-even — a wrong round/sticky/alignment flips
/// a result byte).
#[test]
fn float_arith_routines_simulate_against_rust_reference() {
    // (routine, a, b, label) — the reference is Rust's f32 op.
    let cases: &[(&str, f32, f32, &str)] = &[
        ("__add_f32", 0.5, 0.25, "0.5+0.25"),
        ("__add_f32", 1.0, 1.0, "1.0+1.0"),
        ("__sub_f32", 2.0, 1.0, "2.0-1.0"),
        // The RNE case: 0.1f32 + 0.2f32 = 0x3E99999A (not 0.3's exact
        // neighbors — the round bit + sticky decide the 0x99999A).
        ("__add_f32", 0.1, 0.2, "0.1+0.2"),
        ("__sub_f32", 1.0, 0.5, "1.0-0.5"),
        ("__mul_f32", 2.5, 2.0, "2.5*2.0"),
        // 3.0 * 0.33333334: the 24x24 product's low bits round up to 1.0.
        ("__mul_f32", 3.0, 0.33333334, "3.0*0.33333334"),
        // 0x3C53CE8B * 0x3C53CE8B: the low sum crosses 2^23 mid-product, so
        // the bit-23 carry into the high part (m) fires — the carry path
        // the two cases above never set (their low sums stay below 2^23).
        // Expected 0x392F3E20 from Rust's f32.
        (
            "__mul_f32",
            f32::from_bits(0x3C53_CE8B),
            f32::from_bits(0x3C53_CE8B),
            "mul low sum crosses 2^23",
        ),
        ("__div_f32", 1.0, 4.0, "1.0/4.0"),
        // The load-bearing RNE: 1.0/3.0 = 0x3EAAAAAB (the guard bit 1 with
        // the sticky rounds the 0xAAAAAA mantissa up).
        ("__div_f32", 1.0, 3.0, "1.0/3.0"),
        ("__div_f32", 3.0, 2.0, "3.0/2.0"),
        // 1.0/2^126 = 2^-126 = 0x00800000 exactly — the smallest NORMAL, the
        // exp-1 bit pattern the cmp both-zero check must not read as zero.
        ("__div_f32", 1.0, 2f32.powi(126), "1.0/2^126"),
        ("__sub_f32", 0.5, 0.5, "0.5-0.5"), // exact zero, signs equal
        // 1.0 + (-1.0) = +0 (the sa & sb zero sign rule).
        ("__add_f32", 1.0, -1.0, "1.0-1.0"),
        // The M15 float-differential regression (a hand-picked case, not a
        // seed-0 corpus program): the SUBTRACT path's RNE must account for
        // the alignment's lost bits SUBTRACTING from the difference — the
        // exact result is (ma - mb) - frac, so the rounding mirrors the add
        // path's (round DOWN under round && (sticky || LSB)). Before the
        // fix -2.25 - (-0.0015344163) returned 0xC00FE6DE instead of the
        // RNE 0xC00FE6DC, and 1.0 - 0.99999994 returned 2^-23 instead of
        // 2^-24.
        (
            "__sub_f32",
            -2.25,
            -0.0015344163,
            "sub aligned sticky frac > 1/2",
        ),
        (
            "__sub_f32",
            1.0,
            f32::from_bits(0x3F7F_FFFF),
            "sub aligned frac = 1/2",
        ),
        (
            "__sub_f32",
            1.0,
            f32::from_bits(0x3EFF_FFFD),
            "sub aligned frac = 1/4",
        ),
        (
            "__sub_f32",
            1.0,
            f32::from_bits(0x3EFF_FFFF),
            "sub aligned frac = 3/4 (tie to even)",
        ),
        // 0.0 + x = x; x * 0.0 = +/-0 (the zero-operand shortcuts).
        ("__add_f32", 0.0, 5.0, "0.0+5.0"),
        ("__mul_f32", 0.0, 5.0, "0.0*5.0"),
        ("__mul_f32", -0.0, 5.0, "-0.0*5.0"),
        // div-by-zero -> the deterministic +/-infinity (0x7F800000).
        ("__div_f32", 1.0, 0.0, "1.0/0.0"),
        ("__div_f32", -1.0, 0.0, "-1.0/0.0"),
    ];
    for &(name, a, b, label) in cases {
        let (ir, map) = float_routine_module(name);
        let mut seed = Vec::new();
        for (i, by) in f32_le(a).iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            seed.push((0x24 + i as u16, *by));
        }
        let want = match name {
            "__add_f32" => f32_le(a + b),
            "__sub_f32" => f32_le(a - b),
            "__mul_f32" => f32_le(a * b),
            "__div_f32" => f32_le(a / b),
            _ => unreachable!(),
        };
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
        assert_eq!(
            got, want,
            "{label} ({name}): {got:02X?} must be {want:02X?}"
        );
    }
}

/// The conversion routines: round trips u32/i32 <-> f32 and the truncating
/// fptoui/fptosi. The reference is Rust's f32/int conversions.
#[test]
fn float_conversion_routines_simulate_against_rust_reference() {
    // (routine, input bytes, expected result bytes, label)
    let cases: &[(&str, [u8; 4], [u8; 4], &str)] = &[
        // 12345 -> 12345.0f32 = 0x4640E400.
        (
            "__uitofp_f32",
            12345u32.to_le_bytes(),
            f32_le(12345.0),
            "uitofp 12345",
        ),
        ("__uitofp_f32", 0u32.to_le_bytes(), f32_le(0.0), "uitofp 0"),
        ("__uitofp_f32", 1u32.to_le_bytes(), f32_le(1.0), "uitofp 1"),
        // -7 -> -7.0f32 = 0xC0E00000.
        (
            "__sitofp_f32",
            (-7i32).to_le_bytes(),
            f32_le(-7.0),
            "sitofp -7",
        ),
        ("__sitofp_f32", 0u32.to_le_bytes(), f32_le(0.0), "sitofp 0"),
        // 12345.0 -> 12345; 100.0 -> 100 (truncating).
        (
            "__fptoui_f32",
            f32_le(12345.0),
            12345u32.to_le_bytes(),
            "fptoui 12345.0",
        ),
        (
            "__fptoui_f32",
            f32_le(0.5),
            0u32.to_le_bytes(),
            "fptoui 0.5 truncates",
        ),
        (
            "__fptosi_f32",
            f32_le(-7.0),
            (-7i32).to_le_bytes(),
            "fptosi -7.0",
        ),
        (
            "__fptosi_f32",
            f32_le(100.0),
            100i32.to_le_bytes(),
            "fptosi 100.0",
        ),
        (
            "__fptosi_f32",
            f32_le(-0.5),
            0i32.to_le_bytes(),
            "fptosi -0.5 truncates to 0",
        ),
    ];
    for &(name, inv, want, label) in cases {
        let (ir, map) = float_routine_unary_module(name);
        let mut seed = Vec::new();
        for (i, by) in inv.iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
        assert_eq!(
            got, want,
            "{label} ({name}): {got:02X?} must be {want:02X?}"
        );
    }
}

/// The IEEE754 edge-case suite (issue #11): NaN, infinities, and denormals
/// as OPERANDS of the arithmetic routines, compared bit-for-bit against
/// Rust's f32 (the IEEE default). The existing cases cover the normal range
/// and the deterministic div-by-zero; these cover the operand classes the
/// routines handle "minimally" — the probe found the exact gaps:
///   - add/sub: inf + -inf must be NaN (not 0); denormal + denormal must
///     be the denormal sum (not 0)
///   - mul: inf * 2 must be inf (not 0); inf * 0 must be NaN (not 0);
///     denormal * 2 must be the denormal product (not 0)
///   - div: 1.0 / NaN must be NaN (not garbage); inf / 2 must be inf (not
///     1.7e38); 1.0 / inf must be 0 (not inf); 0.0 / 0.0 must be NaN (not
///     0); denormal / 2 must be the denormal quotient (not 0)
#[test]
fn float_arith_routines_handle_ieee_edge_operands() {
    // (routine, a, b, label) — the reference is Rust's f32 op.
    let cases: &[(&str, f32, f32, &str)] = &[
        // ---- add/sub: inf + -inf = NaN; inf + finite = inf ----
        (
            "__add_f32",
            f32::INFINITY,
            f32::NEG_INFINITY,
            "inf + -inf = NaN",
        ),
        ("__sub_f32", f32::INFINITY, f32::INFINITY, "inf - inf = NaN"),
        ("__add_f32", f32::INFINITY, 1.0, "inf + 1.0 = inf"),
        ("__add_f32", 1.0, f32::INFINITY, "1.0 + inf = inf"),
        ("__add_f32", f32::NEG_INFINITY, 1.0, "-inf + 1.0 = -inf"),
        // ---- add: denormal operands (exp 0, nonzero mantissa) ----
        // The max denormal 0x007FFFFF + 1.0 = 1.0 (the denormal is
        // swallowed by the alignment — the result is exact).
        (
            "__add_f32",
            f32::from_bits(0x007F_FFFF),
            1.0,
            "max denormal + 1.0 = 1.0",
        ),
        // The min denormal + itself = 2^-149 (0x00000002) — the sum of two
        // denormals is a denormal, NOT 0.
        (
            "__add_f32",
            f32::from_bits(0x0000_0001),
            f32::from_bits(0x0000_0001),
            "min denormal + min denormal",
        ),
        // A denormal + a normal that swallows it exactly: 2^-149 + 2^-126
        // = 2^-126 (the denormal is below the normal's ulp).
        (
            "__add_f32",
            f32::from_bits(0x0000_0001),
            f32::from_bits(0x0080_0000),
            "min denormal + min normal",
        ),
        // ---- mul: inf * finite = inf; inf * 0 = NaN; denormal * 2 ----
        ("__mul_f32", f32::INFINITY, 2.0, "inf * 2.0 = inf"),
        ("__mul_f32", f32::INFINITY, 0.0, "inf * 0.0 = NaN"),
        ("__mul_f32", f32::NEG_INFINITY, 0.0, "-inf * 0.0 = NaN"),
        // The max denormal * 2 = 0x00FFFFFE (a denormal — the product of
        // two denormals/normals that underflow stays denormal, not 0).
        (
            "__mul_f32",
            f32::from_bits(0x007F_FFFF),
            2.0,
            "max denormal * 2.0",
        ),
        // ---- div: NaN propagates; inf / finite = inf; finite / inf = 0;
        //      0/0 = NaN; denormal / 2 ----
        ("__div_f32", 1.0, f32::NAN, "1.0 / NaN = NaN"),
        ("__div_f32", f32::NAN, 1.0, "NaN / 1.0 = NaN"),
        ("__div_f32", f32::INFINITY, 2.0, "inf / 2.0 = inf"),
        ("__div_f32", 1.0, f32::INFINITY, "1.0 / inf = 0"),
        ("__div_f32", 0.0, 0.0, "0.0 / 0.0 = NaN"),
        ("__div_f32", f32::INFINITY, 0.0, "inf / 0.0 = inf"),
        ("__div_f32", 0.0, f32::INFINITY, "0.0 / inf = 0"),
        (
            "__div_f32",
            f32::from_bits(0x007F_FFFF),
            2.0,
            "max denormal / 2.0",
        ),
    ];
    for &(name, a, b, label) in cases {
        let (ir, map) = float_routine_module(name);
        let mut seed = Vec::new();
        for (i, by) in f32_le(a).iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            seed.push((0x24 + i as u16, *by));
        }
        let want = match name {
            "__add_f32" => f32_le(a + b),
            "__sub_f32" => f32_le(a - b),
            "__mul_f32" => f32_le(a * b),
            "__div_f32" => f32_le(a / b),
            _ => unreachable!(),
        };
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
        // NaN results: any NaN bit pattern is acceptable (Rust's f32
        // produces a canonical quiet NaN; the routine may produce a
        // different quiet NaN — the CLASS must match).
        let got_f = f32::from_bits(u32::from_le_bytes(got.clone().try_into().unwrap()));
        let want_f = f32::from_bits(u32::from_le_bytes(want.clone().try_into().unwrap()));
        let ok = got == want || (got_f.is_nan() && want_f.is_nan());
        assert!(
            ok,
            "{label} ({name}): {got:02X?} ({got_f:?}) must be {want:02X?} ({want_f:?})"
        );
    }
}

/// The conversion routines' IEEE edge operands (issue #11): NaN/inf/denormal
/// inputs to fptoui/fptosi must saturate deterministically (the C/LLVM
/// contract: out-of-range conversions are poison, but the routine's clamp
/// is the deterministic behavior the PIC side must keep), and uitofp/sitofp
/// of the full u32/i32 range must round correctly at the top end.
#[test]
fn float_conversion_routines_handle_ieee_edge_operands() {
    // (routine, input bytes, expected result bytes, label) — the expected
    // values are the routine's documented deterministic clamps (LLVM's
    // fptoui/fptosi of NaN/inf are poison, so the host cannot be the
    // oracle here; the clamp contract is asserted directly).
    let cases: &[(&str, [u8; 4], [u8; 4], &str)] = &[
        // fptoui of NaN -> 0xFFFFFFFF (the e >= 159 clamp).
        (
            "__fptoui_f32",
            f32_le(f32::NAN),
            0xFFFF_FFFFu32.to_le_bytes(),
            "fptoui NaN clamps",
        ),
        // fptoui of +inf -> 0xFFFFFFFF.
        (
            "__fptoui_f32",
            f32_le(f32::INFINITY),
            0xFFFF_FFFFu32.to_le_bytes(),
            "fptoui +inf clamps",
        ),
        // fptoui of -inf -> 0xFFFFFFFF (the sign is ignored for fptoui).
        (
            "__fptoui_f32",
            f32_le(f32::NEG_INFINITY),
            0xFFFF_FFFFu32.to_le_bytes(),
            "fptoui -inf clamps",
        ),
        // fptosi of +inf -> 0x7FFFFFFF (the positive clamp).
        (
            "__fptosi_f32",
            f32_le(f32::INFINITY),
            0x7FFF_FFFFu32.to_le_bytes(),
            "fptosi +inf clamps",
        ),
        // fptosi of -inf -> 0x80000000 (the negative clamp).
        (
            "__fptosi_f32",
            f32_le(f32::NEG_INFINITY),
            0x8000_0000u32.to_le_bytes(),
            "fptosi -inf clamps",
        ),
        // fptosi of NaN -> 0x80000000 (the negative clamp — the sign bit
        // of the NaN is 0, so the positive clamp 0x7FFFFFFF would be the
        // deterministic choice; the routine's e >= 158 path with sign 0
        // gives 0x7FFFFFFF).
        (
            "__fptosi_f32",
            f32_le(f32::NAN),
            0x7FFF_FFFFu32.to_le_bytes(),
            "fptosi NaN clamps",
        ),
        // fptoui of a denormal -> 0 (e == 0).
        (
            "__fptoui_f32",
            f32_le(f32::from_bits(0x0000_0001)),
            0u32.to_le_bytes(),
            "fptoui denormal -> 0",
        ),
        // fptosi of a denormal -> 0.
        (
            "__fptosi_f32",
            f32_le(f32::from_bits(0x8000_0001)),
            0u32.to_le_bytes(),
            "fptosi -denormal -> 0",
        ),
        // uitofp of u32::MAX = 4294967295 -> 0x4F7FFFFF (the nearest float
        // — RNE: 2^32 - 2^23 is representable, the next float up is 2^32).
        (
            "__uitofp_f32",
            0xFFFF_FFFFu32.to_le_bytes(),
            f32_le(4294967295.0),
            "uitofp u32::MAX",
        ),
        // sitofp of i32::MIN = -2147483648 -> -2^31 = 0xCF000000.
        (
            "__sitofp_f32",
            0x8000_0000u32.to_le_bytes(),
            f32_le(-2147483648.0),
            "sitofp i32::MIN",
        ),
    ];
    for &(name, inv, want, label) in cases {
        let (ir, map) = float_routine_unary_module(name);
        let mut seed = Vec::new();
        for (i, by) in inv.iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
        assert_eq!(
            got, want,
            "{label} ({name}): {got:02X?} must be {want:02X?}"
        );
    }
}

/// The narrow-source conversion CALL ABI: `sitofp`/`uitofp` of i8/i16
/// sources go through `call @__sitofp_f32`/`@__uitofp_f32`, whose `val`
/// param is a fixed 4-byte slot — but the caller copies only the source's
/// own width (2/1 bytes), leaving the high bytes STALE. Pre-fix, an i16
/// `sitofp` reading leftover high bytes (e.g. 0x41, 0x1C from an earlier
/// fptosi) started its leading-1 search at bit 30 and produced exp 157
/// instead of 130 (the M15 acceptance caught this: out2 was 0x4E823800
/// instead of 0x41100000). The ABI now fills the remainder — sign-extend
/// for __sitofp_f32, zero-extend for __uitofp_f32 — so the recipe always
/// sees a proper i32. The sim seeds garbage into the slot's high bytes
/// before the call to prove the caller overwrites them.
#[test]
fn narrow_conversion_sources_sign_and_zero_extend_through_the_call_abi() {
    use ir::Inst;
    fn inst_dst_ty(i: &Inst) -> Option<(String, ir::Ty)> {
        match i {
            Inst::Load(l) => Some((l.dst.clone(), l.ty)),
            Inst::Bin(b) => Some((b.dst.clone(), b.ty)),
            Inst::Zext(z) => Some((z.dst.clone(), z.to)),
            Inst::Sext(s) => Some((s.dst.clone(), s.to)),
            Inst::Trunc(t) => Some((t.dst.clone(), t.to)),
            Inst::IntToPtr(p) => Some((p.dst.clone(), p.to)),
            Inst::Icmp(c) => Some((c.dst.clone(), ir::Ty::I1)),
            Inst::Select(s) => Some((s.dst.clone(), s.ty)),
            Inst::Call(c) => c
                .dst
                .clone()
                .map(|d| (d, c.ty.expect("isel: valued call ty"))),
            Inst::Phi(p) => Some((p.dst.clone(), p.ty)),
            Inst::Freeze(f) => Some((f.dst.clone(), f.ty)),
            Inst::Alloca(a) => Some((a.dst.clone(), ir::Ty::I8)), // __scr
            _ => None,
        }
    }
    fn module_map(m: &ir::Module) -> Vec<(String, u16)> {
        let mut map = Vec::new();
        let mut addr = 0x20u16;
        for g in &m.globals {
            map.push((g.name.clone(), addr));
            addr += g.ty.bytes() as u16;
        }
        for f in &m.funcs {
            for p in &f.params {
                map.push((format!("{}::{}", f.name, p.name), addr));
                addr += u16::from(p.width);
            }
            for b in &f.blocks {
                for i in &b.insts {
                    if let Some((d, t)) = inst_dst_ty(i) {
                        map.push((format!("{}::{d}", f.name), addr));
                        addr += t.bytes() as u16;
                    }
                }
            }
        }
        map
    }
    // (source width, op, input bytes, expected float bytes, label) — the
    // expected values are Rust's own f32 conversions (the arithmetic
    // authority). The i16/i8 negatives exercise the sign extension; the
    // unsigned values the zero extension.
    let cases: &[(&str, &str, [u8; 2], [u8; 4], &str)] = &[
        (
            "i16",
            "sitofp",
            (-7i16).to_le_bytes(),
            f32_le(-7.0),
            "sitofp i16 -7",
        ),
        (
            "i16",
            "sitofp",
            9i16.to_le_bytes(),
            f32_le(9.0),
            "sitofp i16 9",
        ),
        (
            "i16",
            "uitofp",
            65529u16.to_le_bytes(),
            f32_le(65529.0),
            "uitofp i16 65529",
        ),
        (
            "i8",
            "sitofp",
            [0xF9, 0x00],
            f32_le(-7.0),
            "sitofp i8 -7 (0xF9)",
        ),
        (
            "i8",
            "uitofp",
            [0xF9, 0x00],
            f32_le(249.0),
            "uitofp i8 249 (0xF9)",
        ),
        ("i8", "sitofp", [0x7F, 0x00], f32_le(127.0), "sitofp i8 127"),
    ];
    for &(width, op, inv, want, label) in cases {
        let src = format!(
            "global inv {width}\n\
             global out float\n\
             fn main(void) ()\n\
               block entry:\n\
                 %v = load {width} @inv\n\
                 %r = {op} {width} %v to float\n\
                 store float %r @out\n\
                 ret void\n"
        );
        let m = legalize::legalize(parse(&src));
        let map = module_map(&m);
        let asm = select(&PIC16F877A, &m, &addrs(&map_refs(&map)));
        let words = asm::assemble(&asm);
        let mut p = pic14_sim::Pic14::new(words);
        for (i, by) in inv.iter().enumerate() {
            p.ram_mut()[0x20 + i] = *by;
        }
        // Garbage in the routine's val-slot high bytes (the pre-fix stale
        // data): a wrong exp (157 vs 130) must NOT come back.
        let val = *map
            .iter()
            .find(|(k, _)| k == &format!("__{op}_f32::val"))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no val slot for __{op}_f32 in map"));
        p.ram_mut()[val as usize + 2] = 0x1C;
        p.ram_mut()[val as usize + 3] = 0x41;
        let out_addr = *map
            .iter()
            .find(|(k, _)| k == "out")
            .map(|(_, v)| v)
            .expect("out global in map");
        p.run(200_000);
        assert!(p.halted(), "{label}: program must SLEEP-halt:\n{asm}");
        let mut got = [0u8; 4];
        for i in 0..4 {
            got[i] = p.ram()[out_addr as usize + i];
        }
        assert_eq!(got, want, "{label}: {got:02X?} must be {want:02X?}:\n{asm}");
    }
}

/// `__cmp_f32` returns the tri-state byte 0=equal / 1=a<b / 2=a>b /
/// 3=unordered — including the -0 == +0 case and the sign-magnitude
/// ordering.
#[test]
fn cmp_f32_simulates_tristate_byte() {
    let cases: &[(&str, f32, f32, u8)] = &[
        ("eq", 1.0, 1.0, 0),
        ("lt", 1.0, 2.0, 1),
        ("gt", 2.0, 1.0, 2),
        ("-0 == +0", -0.0, 0.0, 0),
        ("+0 == -0", 0.0, -0.0, 0),
        ("neg lt", -2.0, 1.0, 1),
        ("neg gt", -1.0, -2.0, 2),
        ("neg eq", -3.0, -3.0, 0),
        ("pos vs neg", 1.0, -2.0, 2),
        // The smallest NORMALs (full 8-bit exp 1, the LSB in b2 bit 7): the
        // both-zero shortcut must NOT swallow them (pre-fix these returned 0).
        ("smallest normal gt +0", f32::from_bits(0x00800000), 0.0, 2),
        (
            "-smallest normal lt +smallest normal",
            f32::from_bits(0x80800000),
            f32::from_bits(0x00800000),
            1,
        ),
        (
            "smallest normal lt next normal",
            f32::from_bits(0x00800000),
            f32::from_bits(0x00C00000),
            1,
        ),
        ("NaN a", f32::NAN, 1.0, 3),
        ("NaN b", 1.0, f32::NAN, 3),
        ("NaN both", f32::NAN, f32::NAN, 3),
    ];
    for &(label, a, b, want) in cases {
        let (ir, map) = float_routine_module("__cmp_f32");
        let mut seed = Vec::new();
        for (i, by) in f32_le(a).iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            seed.push((0x24 + i as u16, *by));
        }
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, 1)[0];
        assert_eq!(got, want, "{label}: cmp_f32({a}, {b}) must be {want}");
    }
}

/// The end-to-end reachability: __div_f32(1.0, 2^126) lands bit-exactly on
/// the smallest normal 0x00800000, and a following __cmp_f32 of that quotient
/// against +0.0 must report a > b (2). Pre-fix the both-zero shortcut read
/// the exp-1 pattern as zero and returned 0.
#[test]
fn cmp_f32_sees_smallest_normal_produced_by_div() {
    // div first: 1.0 / 2^126 = 2^-126 = 0x00800000 (exact).
    let (ir, map) = float_routine_module("__div_f32");
    let mut seed = Vec::new();
    for (i, by) in f32_le(1.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *by));
    }
    for (i, by) in f32_le(2f32.powi(126)).iter().enumerate() {
        seed.push((0x24 + i as u16, *by));
    }
    let q = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(
        q,
        f32_le(f32::from_bits(0x00800000)),
        "div_f32(1.0, 2^126) must be 0x00800000: {q:02X?}"
    );
    // cmp the quotient against +0.0 -> a > b (2).
    let (ir, map) = float_routine_module("__cmp_f32");
    let mut seed = Vec::new();
    for (i, by) in q.iter().enumerate() {
        seed.push((0x20 + i as u16, *by));
    }
    for (i, by) in f32_le(0.0).iter().enumerate() {
        seed.push((0x24 + i as u16, *by));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 1)[0];
    assert_eq!(got, 2, "cmp_f32(2^-126, 0.0) must be 2 (a > b)");
}

/// Every float routine emits a real recipe body — the label, the recipe
/// instructions, and a RETURN (never an empty label falling through into
/// the next function — the M14 loud-panic gate the empty-label hazard used
/// to guard). The `pats` are load-bearing idioms at the contract addresses.
#[test]
fn float_routines_emit_recipe_bodies() {
    let cases: &[(&str, &[&str])] = &[
        (
            "__add_f32",
            &[
                "RRF 0x4C, F",   // ma2 = __scr+4: the alignment right shift
                "ADDWF 0x4A, F", // ma0 = __scr+2: the 24-bit add
                "SUBWF 0x53, W", // cnt = __scr+11: the alignment clamp (31)
                "MOVWF 0x74",    // the retval b3 byte
            ],
        ),
        (
            "__sub_f32",
            &[
                "XORLW 0x80",    // the sign flip
                "SUBWF 0x4A, F", // the 24-bit subtract
                "MOVWF 0x74",
            ],
        ),
        (
            "__mul_f32",
            &[
                "RLF 0x4B, F",   // bk0 = __scr+3: the multiplier test shift
                "ADDWF 0x4F, F", // m0 = __scr+7: the product accumulation
                "RRF 0x40, F",   // the addend right shift in the a param slot
                "MOVWF 0x74",
            ],
        ),
        (
            "__div_f32",
            &[
                "RLF 0x40, F",   // num <<= 1 (the dividend = the a param slot)
                "SUBWF 0x4B, F", // rem0 = __scr+3: the restoring subtract
                "BSF 0x40, 0",   // the quotient bit
                "MOVWF 0x74",
            ],
        ),
        (
            "__uitofp_f32",
            &[
                "RLF 0x40, F", // the leading-1 shift
                "SUBLW 0x9E",  // e = 158 - cnt
                "MOVWF 0x74",
            ],
        ),
        (
            "__sitofp_f32",
            &[
                "ANDLW 0x80",   // the sign save
                "COMF 0x40, F", // the abs (negate in place)
                "SUBLW 0x9E",
                "MOVWF 0x74",
            ],
        ),
        (
            "__fptoui_f32",
            &[
                "SUBLW 0x96",  // cnt = 150 - e
                "RRF 0x48, F", // m2 = __scr+4 (unary layout: __scr at 0x44)
                "MOVWF 0x74",
            ],
        ),
        (
            "__fptosi_f32",
            &[
                "SUBLW 0x96",
                "COMF 0x48, F", // the sign negate (m2 = __scr+4, unary layout)
                "MOVWF 0x74",
            ],
        ),
        (
            "__cmp_f32",
            &[
                "SUBLW 0x7F",    // the NaN exp check
                "SUBWF 0x40, W", // the 4-byte magnitude compare
                "MOVLW 0x03",    // the unordered result
            ],
        ),
    ];
    for &(name, pats) in cases {
        let (ir, map) = if name == "__cmp_f32" {
            float_routine_module(name)
        } else if name.ends_with("_f32")
            && !matches!(name, "__add_f32" | "__sub_f32" | "__mul_f32" | "__div_f32")
        {
            // unary conversion routines
            let (ir, map) = float_routine_unary_module(name);
            (ir, map)
        } else {
            float_routine_module(name)
        };
        let asm = select(&PIC16F877A, &parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body
            .split("main:")
            .next()
            .expect("main label after routine");
        assert!(
            body.contains("    RETURN"),
            "{name} body must end in RETURN, not fall through:\n{asm}"
        );
        for p in pats {
            assert!(asm.contains(p), "{name} must contain `{p}`:\n{asm}");
        }
    }
}

/// The fcmp end-to-end: a module with `fcmp` predicates is parsed, lowered
/// by legalize into `call i8 @__cmp_f32` + the per-predicate icmp/select
/// tree, selected, assembled, and run in pic14_sim — the tri-state byte
/// feeds the tree and the final i1 lands in `out`. The address map is built
/// from the legalized module (globals at 0x20+, then each function's params
/// and inst dsts in order), so the lowered fresh names need no hardcoding.
#[test]
fn fcmp_predicates_materialize_end_to_end() {
    use ir::Inst;
    fn inst_dst_ty(i: &Inst) -> Option<(String, ir::Ty)> {
        match i {
            Inst::Load(l) => Some((l.dst.clone(), l.ty)),
            Inst::Bin(b) => Some((b.dst.clone(), b.ty)),
            Inst::Zext(z) => Some((z.dst.clone(), z.to)),
            Inst::Sext(s) => Some((s.dst.clone(), s.to)),
            Inst::Trunc(t) => Some((t.dst.clone(), t.to)),
            Inst::IntToPtr(p) => Some((p.dst.clone(), p.to)),
            Inst::Icmp(c) => Some((c.dst.clone(), ir::Ty::I1)),
            Inst::Select(s) => Some((s.dst.clone(), s.ty)),
            Inst::Call(c) => c
                .dst
                .clone()
                .map(|d| (d, c.ty.expect("isel: valued call ty"))),
            Inst::Phi(p) => Some((p.dst.clone(), p.ty)),
            Inst::Freeze(f) => Some((f.dst.clone(), f.ty)),
            Inst::Alloca(a) => Some((a.dst.clone(), ir::Ty::I8)), // __scr
            _ => None,
        }
    }
    fn module_map(m: &ir::Module) -> Vec<(String, u16)> {
        let mut map = Vec::new();
        let mut addr = 0x20u16;
        for g in &m.globals {
            map.push((g.name.clone(), addr));
            addr += g.ty.bytes() as u16;
        }
        for f in &m.funcs {
            for p in &f.params {
                map.push((format!("{}::{}", f.name, p.name), addr));
                addr += u16::from(p.width);
            }
            for b in &f.blocks {
                for i in &b.insts {
                    if let Some((d, t)) = inst_dst_ty(i) {
                        map.push((format!("{}::{d}", f.name), addr));
                        addr += t.bytes() as u16;
                    }
                }
            }
        }
        map
    }
    let cases: &[(&str, f32, f32, u8)] = &[
        // oeq = (c==0)
        ("oeq", 1.0, 1.0, 1),
        ("oeq", 1.0, 2.0, 0),
        // olt = (c==1)
        ("olt", 1.0, 2.0, 1),
        ("olt", 2.0, 1.0, 0),
        // one = (c==1) || (c==2) — the OR select materialization
        ("one", 1.0, 2.0, 1),
        ("one", 2.0, 1.0, 1),
        ("one", 1.0, 1.0, 0),
        // ord = (c!=3)
        ("ord", 1.0, 2.0, 1),
        ("ord", f32::NAN, 1.0, 0),
        // uno = (c==3)
        ("uno", f32::NAN, 1.0, 1),
        ("uno", 1.0, 2.0, 0),
    ];
    for &(pred, a, b, want) in cases {
        let src = format!(
            "global ina float\n\
             global inb float\n\
             global out i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %x = load float @ina\n\
                 %y = load float @inb\n\
                 %r = fcmp {pred} float %x %y\n\
                 store i8 %r @out\n\
                 ret void\n"
        );
        let m = legalize::legalize(parse(&src));
        let map = module_map(&m);
        let asm = select(&PIC16F877A, &m, &addrs(&map_refs(&map)));
        let words = asm::assemble(&asm);
        let mut p = pic14_sim::Pic14::new(words);
        for (i, by) in f32_le(a).iter().enumerate() {
            p.ram_mut()[0x20 + i] = *by;
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            p.ram_mut()[0x24 + i] = *by;
        }
        p.run(200_000);
        assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
        assert_eq!(
            p.ram()[0x28],
            want,
            "fcmp {pred}({a}, {b}) must be {want}:\n{asm}"
        );
    }
}

#[test]
fn pointer_param_high_byte_reads_without_a_phantom_carry() {
    // Byte 1 of a pointer param's value is a plain read: byte 0 was a bare
    // MOVF and produced no carry, so propagating one both adds a phantom 1
    // and clobbers the carry the surrounding 16-bit compare is about to test
    // (memmove's `d > s` always answered false).
    let m = parse(
        "global out i8\nfn f(void) (0=ptr, 1=ptr)\n  block entry:\n\
           %c = icmp ugt i16 %0 %1\n    store i8 %c @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("out", 0x21),
        ("f::0", 0x25),
        ("f::1", 0x27),
        ("f::c", 0x29),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        !asm.contains("ADDLW 0x01"),
        "a k=0 pointer param read must not propagate a carry:\n{asm}"
    );
}

/// An indirect call through a function pointer lowers to a compare-and-call
/// chain over the candidate set: each candidate's two address bytes are
/// compared against the fp value, the matched arm CALLs, and an unmatched fp
/// falls into a deterministic trap (epic-cc#73).
#[test]
fn indirect_call_emits_compare_and_call_chain() {
    let m = parse(
        "fn f0(void) ()\n  block entry:\n    ret void\n\
         fn f1(void) ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           call void %3() callees f0 f1\n    ret void\n",
    );
    let addrs = addrs(&[("main::3", 0x2B)]);
    let asm = select(&PIC16F877A, &m, &addrs);
    assert!(
        asm.contains("XORLW LOW(f0)"),
        "compare fp lo against f0:\n{asm}"
    );
    assert!(
        asm.contains("XORLW HIGH(f0)"),
        "compare fp hi against f0:\n{asm}"
    );
    assert!(
        asm.contains("XORLW LOW(f1)"),
        "compare fp against f1:\n{asm}"
    );
    assert!(asm.contains("    CALL f0"), "CALL f0:\n{asm}");
    assert!(asm.contains("    CALL f1"), "CALL f1:\n{asm}");
    assert!(asm.contains("GOTO tmp"), "trap loop:\n{asm}");
}

#[test]
fn single_entry_table_crossing_window_is_256_aligned() {
    // Issue #138: a <= 255-byte table whose natural base would cross its
    // 256-byte window used to panic in the assembler's `.table` assert (a
    // 60-byte table at base 0x1EA: LOW 0xEA + 60 = 0x126 > 0x100). The
    // emitter now folds `.align 256` before the `.table` directive, the
    // exact chunked-branch alignment, so placement never decides the fit.
    // main is padded so the table's natural base lands at 0x1EA: layout =
    // goto (1) + __start (4) + main (479) + t reader (6) -> base 0x1EA,
    // aligned to 0x200.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n{}    ret void\n",
        pad_body(156)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 60),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
        ("main::a", 0x27),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    // The crossing is real: without the align the base would sit at LOW
    // 0xEA and the assembler's `.table` assert would fire (the regression
    // this test pins).
    let base = label_addr(&asm, "t");
    assert!(
        base & 0xFF == 0,
        "crossing base must be 256-aligned (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        asm.contains("    .align 256\n    .table t 60"),
        "the .align 256 must precede the .table directive:\n{asm}"
    );
    // And the program really builds and runs: in = 3 -> table[3] = 3.
    verify_page_fit(&m, &asm);
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 3)], 0x21),
        3,
        "table[3] = 3 through the aligned window-2 reader:\n{asm}"
    );
}

#[test]
fn single_entry_table_within_window_emits_no_align() {
    // A <= 255-byte table whose natural base already fits its window must
    // not gain a pad: `.align 256` would waste up to 255 words of flash for
    // nothing. main is padded so the table's natural base lands in window
    // 0 (LOW + 60 <= 0x100), far from any crossing.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n{}    ret void\n",
        pad_body(10)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 60),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
        ("main::a", 0x27),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    let base = label_addr(&asm, "t");
    assert!(
        (base & 0xFF) + 60 <= 0x100,
        "table must sit in window 0 (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        !asm.contains("    .align 256"),
        "no pad for a table that fits its window:\n{asm}"
    );
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 5)], 0x21),
        5,
        "table[5] = 5:\n{asm}"
    );
}

#[test]
fn config_table_crossing_window_top_is_256_aligned() {
    // epic-cc#121: the 68-byte `__epic_config` table placed at base 0xFED
    // (LOW 0xED + 68 > 0x100) used to panic the assembler's `.table`
    // window assert. The emitter's `window_align` folds `.align 256`
    // before the `.table` directive, so placement never decides the fit.
    // main (26 words) sits in page 0; helper (2023 words) fills page 1 to
    // 0xFE7, so the table's natural base lands at 0xFED, aligned to
    // 0x1000.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n{}    ret void\n\
         fn helper(void) ()\n  block entry:\n{}    ret void\n",
        pad_body(5),
        pad_body(674)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 68),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
        ("main::a", 0x27),
        ("helper::a", 0x28),
    ]);
    let asm = select(&PIC16F877A, &m, &addrs);
    let base = label_addr(&asm, "t");
    assert!(
        base & 0xFF == 0,
        "crossing base must be 256-aligned (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        asm.contains("    .align 256\n    .table t 68"),
        "the .align 256 must precede the .table directive:\n{asm}"
    );
    verify_page_fit(&m, &asm);
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 3)], 0x21),
        3,
        "table[3] = 3 through the aligned reader:\n{asm}"
    );
}
