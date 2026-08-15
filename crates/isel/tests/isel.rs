use isel::select;
use ir::parse;
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
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::1", 0x25), ("main::2", 0x26)]);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x20, W"));
    assert!(asm.contains("MOVWF 0x25"), "%1 must live at its map address 0x25:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"));
    assert!(asm.contains("MOVWF 0x26"), "%2 must live at its map address 0x26:\n{asm}");
    assert!(asm.contains("MOVWF 0x21"));
}

#[test]
fn store_const_emits_movlw_not_movf() {
    let m = parse("global out i8\nfn main(void) ()\n  block entry:\n    store i8 5 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("out".to_string(), 0x21u16);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVLW 0x05"), "expected MOVLW for const store:\n{asm}");
    assert!(asm.contains("MOVWF 0x21"), "expected MOVWF to @out:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
fn add_const_lhs_uses_addlw() {
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %x = add i8 5, %1\n    store i8 %x @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::1", 0x25), ("main::x", 0x26)]);
    let asm = select(&m, &addrs);
    assert!(asm.contains("ADDLW 0x05"), "const-LHS add should use the ADDLW path:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "the result lands at its map address:\n{asm}");
    assert!(!asm.contains("ADDWF 0x05"), "const must not be read as a file register:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
#[should_panic(expected = "only i8/i16 loads supported")]
fn panics_on_i1_load() {
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i1 @in\n    ret void\n");
    select(&m, &HashMap::new());
}

#[test]
#[should_panic(expected = "no slot for main::1")]
fn panics_when_local_address_missing_from_map() {
    // Every local address comes from the map; a missing entry must fail
    // loudly instead of allocating a slot internally.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let loads: String = (0..16).map(|i| format!("    %a{i} = load i8 @g{i}\n")).collect();
    let m = parse(&format!(
        "{globals}global out i16\nfn main(void) ()\n  block entry:\n{loads}    %r = load i16 @out\n    store i16 %r @out\n    ret void\n"
    ));
    let mut addrs: HashMap<String, u16> = (0..16).map(|i| (format!("g{i}"), 0x20 + i)).collect();
    addrs.insert("out".to_string(), 0x30u16);
    for i in 0..16 {
        addrs.insert(format!("main::a{i}"), 0x35 + i);
    }
    addrs.insert("main::r".to_string(), 0x45u16);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVWF 0x45"), "i16 lo should land at map address 0x45:\n{asm}");
    assert!(asm.contains("MOVWF 0x46"), "i16 hi should land at map address 0x46:\n{asm}");
    assert!(asm.contains("MOVF 0x46, W"), "store reads the i16 hi from 0x46:\n{asm}");
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    // %v=0x27, %z lo=0x28 hi=0x29, %t=0x2A.
    assert!(asm.contains("MOVF 0x27, W"), "zext copies v:\n{asm}");
    assert!(asm.contains("MOVWF 0x28"), "zext stores d_lo:\n{asm}");
    assert!(asm.contains("CLRF 0x29"), "zext zeroes d_hi:\n{asm}");
    assert!(asm.contains("MOVF 0x28, W"), "trunc reads z_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "trunc stores d:\n{asm}");
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
    let asm = select(&m, &addrs);
    // %v=0x27, %s lo=0x28 hi=0x29.
    assert!(asm.contains("MOVF 0x27, W"), "sext copies v:\n{asm}");
    assert!(asm.contains("MOVWF 0x28"), "sext stores d_lo:\n{asm}");
    // Sign-fill: test the source's MSB (byte 0 of the i8 lives at 0x27),
    // then fill the high byte with 0xFF (negative) or 0x00 (positive).
    assert!(asm.contains("BTFSS 0x27, 7"), "sext tests v's sign bit:\n{asm}");
    assert!(asm.contains("MOVLW 0xFF"), "sext fills 0xFF when negative:\n{asm}");
    assert!(asm.contains("MOVLW 0x00"), "sext fills 0x00 when positive:\n{asm}");
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
    let asm = select(&m, &map);
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
    let asm = select(&m, &addrs);
    // %a=0x29 (hi 0x2A), %b=0x2B (hi 0x2C), %p=0x2D (hi 0x2E).
    // In block `entry` the copy of %a (ending MOVWF 0x2E) precedes its GOTO.
    assert!(
        asm.contains("MOVWF 0x2E\n    GOTO main_Lmerge"),
        "copy must land before the entry terminator:\n{asm}"
    );
    // In block `thenb` the copy of %b (ending MOVWF 0x2E) precedes its GOTO.
    assert!(
        asm.contains("MOVF 0x2B, W\n    MOVWF 0x2D\n    MOVF 0x2C, W\n    MOVWF 0x2E\n    GOTO main_Lmerge"),
        "copy must land before the thenb terminator:\n{asm}"
    );
    // The merge block reads the phi destination (0x2D lo / 0x2E hi).
    assert!(asm.contains("MOVF 0x2D, W"), "merge reads %p lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x24"), "merge stores %p lo to @out:\n{asm}");
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
    let asm = select(&m, &addrs);
    // %p=0x27, %q=0x28, %a=0x26.
    assert!(
        asm.contains("MOVF 0x26, W\n    MOVWF 0x27\n    MOVF 0x27, W\n    MOVWF 0x28"),
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
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    // %1=0x25, %c=0x26, scratch=0x70 (fixed common RAM).
    assert!(asm.contains("MOVF 0x25, W"), "load a:\n{asm}");
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
    let asm = select(&m, &addrs);
    // %a=0x28/29, %b=0x2A/2B, %c=0x2C, scratch=0x70 (fixed common RAM).
    assert!(asm.contains("MOVF 0x28, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("XORWF 0x2A, W"), "xor b_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "store lo xor to scratch:\n{asm}");
    assert!(asm.contains("MOVF 0x29, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("XORWF 0x2B, W"), "xor b_hi:\n{asm}");
    assert!(asm.contains("IORWF 0x70, W"), "or hi into scratch:\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "store accumulated scratch:\n{asm}");
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    // Collect every emitted label *definition* (e.g. "tmp0:", not "GOTO tmp0").
    let defs: Vec<&str> = asm
        .lines()
        .filter(|l| l.trim_start().starts_with("tmp") && l.ends_with(':'))
        .collect();
    assert_eq!(defs.len(), 4, "two selects -> 4 labels, got {defs:?}:\n{asm}");
    let mut unique = defs.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 4, "fresh labels must be unique across functions, got {defs:?}:\n{asm}");
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
    let asm = select(&m, &addrs);
    // Fixed common RAM: scratch 0x70, retval 0x71/0x72.
    assert!(asm.contains("MOVWF 0x70"), "icmp writes the fixed scratch 0x70:\n{asm}");
    assert!(!asm.contains("MOVWF 0x71") && !asm.contains("MOVWF 0x72"), "no writes to the retval bytes:\n{asm}");
    assert!(asm.contains("MOVWF 0x73"), "the load lands at the map address 0x73:\n{asm}");
    assert!(asm.contains("MOVWF 0x74"), "icmp dst at the map address 0x74:\n{asm}");
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
        },
        ir::Global {
            name: "ram".into(),
            ty: ir::Ty::I8,
            is_const: false,
            size: 8,
            bytes: vec![0; 8],
            addr: None,
        },
        ir::Global {
            name: "table".into(),
            ty: ir::Ty::I8,
            is_const: true,
            size: 4,
            bytes: vec![10, 20, 30, 40],
            addr: None,
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
    let asm = select(&m, &addrs);
    // RAM indirect load (%w = load i8 %p): W = %i; W += 0x21 (base_lo);
    // FSR = W; W = INDF; %w = W.
    assert!(asm.contains("MOVF 0x29, W"), "offset %i:\n{asm}");
    assert!(asm.contains("ADDLW 0x21"), "base_lo of @ram:\n{asm}");
    assert!(asm.contains("MOVWF FSR"), "FSR = base + offset:\n{asm}");
    assert!(asm.contains("MOVF INDF, W"), "load through FSR:\n{asm}");
    assert!(asm.contains("MOVWF 0x2B"), "RAM load dst %w:\n{asm}");
    // RAM indirect store (store i8 %v %p): same FSR setup, then W = %v;
    // INDF = W.
    assert!(asm.contains("MOVF 0x2A, W"), "store value %v:\n{asm}");
    assert!(asm.contains("MOVWF INDF"), "store through FSR:\n{asm}");
    // Const load (%v = load i8 %t): W = %i (index); CALL __read_table;
    // W -> %v.
    assert!(asm.contains("CALL __read_table"), "const load calls the table reader:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "const load dst %v:\n{asm}");
    // The RETLW table itself, after the functions.
    assert!(asm.contains("__read_table:"), "table reader label:\n{asm}");
    assert!(asm.contains("ADDLW LOW(table)"), "index += table base:\n{asm}");
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
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x22, W"), "direct byte-offset load at g+2:\n{asm}");
    assert!(!asm.contains("MOVWF FSR"), "no FSR setup for a constant offset:\n{asm}");
    assert!(asm.contains("MOVWF 0x24"), "store to @out:\n{asm}");
}

#[test]
fn gep_single_term_uses_fsr_fast_path() {
    // %p = gep @a +1 +1*%i: one scale-1 term keeps the M5 fast shape —
    // MOVF %i,W; ADDLW <a_lo + k>; MOVWF FSR — with the constant k folded
    // into the ADDLW literal.
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
    let asm = select(&m, &addrs);
    assert!(
        asm.contains("MOVF 0x29, W\n    ADDLW 0x22\n    MOVWF FSR\n    MOVF INDF, W\n    MOVWF 0x2A"),
        "fast path: FSR = a_lo + k + i (0x21 + 1):\n{asm}"
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
    let asm = select(&m, &addrs);
    // scale-2 term: scratch = 2×%i — %i is reloaded into W before each
    // ADDWF (ADDWF f,W computes W = f + W), then FSR = scratch + a_lo + k.
    assert!(
        asm.contains("MOVLW 0x00\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x23\n    MOVWF FSR"),
        "scaled term accumulates in scratch:\n{asm}"
    );
    // two distinct terms accumulate in order (i then j), same FSR finish.
    assert!(
        asm.contains("MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x23\n    MOVWF FSR"),
        "two-term sum accumulates both terms:\n{asm}"
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
    let asm = select(&m, &addrs(&map));
    // MOVF %j,W must appear once per scale-2 repetition — twice in total.
    assert_eq!(
        asm.matches("MOVF 0x2A, W").count(),
        2,
        "scale-2 term must reload %j into W before each ADDWF:\n{asm}"
    );
    // Full sequence: scratch = i + 2*j = i + j + j, then FSR = a_lo + 1 + scratch.
    assert!(
        asm.contains("MOVLW 0x00\n    MOVWF 0x70\n    MOVF 0x29, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    ADDWF 0x70, W\n    MOVWF 0x70\n    MOVF 0x70, W\n    ADDLW 0x41\n    MOVWF FSR"),
        "scaled multi-term sum accumulates i + 2*j:\n{asm}"
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
    assert_eq!(sim_run(ir, &map, &seed(2, 2), 0x22), 0x17, "a[1+2+4] with i=2, j=2");
    assert_eq!(sim_run(ir, &map, &seed(3, 1), 0x22), 0x16, "a[1+3+2] with i=3, j=1");
}

#[test]
fn sret_param_store_is_indirect_via_slot_contents() {
    // An sret param slot holds the *target address*; a store through it
    // must set FSR from the slot's contents — never treat the slot itself
    // as the destination.
    let m = parse(
        "global v i8\nfn make(i8) (r=sret)\n  block entry:\n\
           %x = load i8 @v\n    %p = gep %r +0\n    store i8 %x %p\n    ret void\n",
    );
    // v=0x20; make's frame: %x=0x25, sret slot r=0x26 (2 bytes).
    let addrs = addrs(&[("v", 0x20), ("make::x", 0x25), ("make::r", 0x26)]);
    let asm = select(&m, &addrs);
    assert!(
        asm.contains("MOVF 0x26, W\n    ADDLW 0x00\n    MOVWF FSR\n    MOVF 0x25, W\n    MOVWF INDF"),
        "FSR comes from the slot contents [r_lo] + k:\n{asm}"
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
    let asm = select(&m, &addrs);
    for i in 0..4u16 {
        assert!(
            asm.contains(&format!("MOVF 0x{:02X}, W\n    MOVWF 0x{:02X}", 0x24 + i, 0x20 + i)),
            "byte {i} copy:\n{asm}"
        );
    }
    assert!(!asm.contains("MOVWF FSR"), "direct globals need no FSR setup:\n{asm}");
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
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVWF 0x27"), "store into buf+2:\n{asm}");
    assert!(asm.contains("MOVF 0x27, W"), "load from buf+2:\n{asm}");
    assert!(!asm.contains("MOVWF FSR"), "constant alloca offset needs no FSR setup:\n{asm}");
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
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x27, W"), "byval field byte at slot+2:\n{asm}");
    assert!(!asm.contains("MOVWF FSR"), "direct byval slot needs no FSR setup:\n{asm}");
}

#[test]
#[should_panic(expected = "bank-0")]
fn panics_on_banked_fsr_base() {
    // FSR reaches only the low 256 bytes (IRP is a later milestone): a
    // dynamic-index base past bank 0 must fail loudly rather than emit an
    // ADDLW literal that cannot express the address.
    let m = parse(
        "global in i8\nglobal far i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @far +0 +1*%i\n    %v = load i8 %p\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("far", 0x120), ("main::i", 0x29), ("main::v", 0x2A)]);
    let _ = select(&m, &addrs);
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
    let _ = select(&m, &HashMap::new());
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
    let asm = select(&m, &addrs(&map));
    // The chain folds k = 1 + 1 = 2 into the fast path's ADDLW literal.
    assert!(
        asm.contains("MOVF 0x29, W\n    ADDLW 0x23\n    MOVWF FSR\n    MOVF INDF, W"),
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
            &[(0x20, 0), (0x21, 0x11), (0x22, 0x22), (0x23, 0x33), (0x24, 0x44)],
            0x25,
        ),
        0x33,
        "a[2+0] with i=0"
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
    assert!(!addrs.contains_key("table"), "const globals have no RAM address");
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    // %a=0x25, %r=0x26.
    assert!(asm.contains("MOVLW 0x05"), "load k into W:\n{asm}");
    assert!(asm.contains("SUBWF 0x25, W"), "a - k via SUBWF a,W:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "store d:\n{asm}");
    // Direction guard: SUBLW would compute k - a (wrong direction).
    assert!(!asm.contains("SUBLW"), "reg-const sub must not use SUBLW:\n{asm}");
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
#[should_panic(expected = "const LHS")]
fn panics_on_sub_const_lhs() {
    // sub is NOT commutative: d = k - a cannot reuse the reg-const lowering
    // (which computes a - k) and must not read a const as a file register.
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i8 @x\n    %r = sub i8 5, %a\n    store i8 %r @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::r", 0x26),
    ]);
    let _ = select(&m, &addrs);
}

#[test]
fn and_i8_uses_andwf_andlw() {
    // reg-reg: MOVF b,W; ANDWF a,W; MOVWF d. reg-const: MOVF a,W; ANDLW k.
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
    let asm = select(&m, &addrs);
    // reg-reg: %a=0x27, %b=0x28, %r1=0x29.
    assert!(asm.contains("MOVF 0x28, W"), "load b:\n{asm}");
    assert!(asm.contains("ANDWF 0x27, W"), "a & b:\n{asm}");
    assert!(asm.contains("MOVWF 0x29"), "store d1:\n{asm}");
    // reg-const: %a=0x27, %r2=0x2A.
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    assert!(asm.contains("IORLW 0x05"), "const-LHS or should use the IORLW path:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "the result lands at its map address:\n{asm}");
    assert!(!asm.contains("IORWF 0x05"), "const must not be read as a file register:\n{asm}");
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
    let asm = select(&m, &addrs);
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
        assert_eq!(p.ram()[0x24], 7, "reg-const sub must compute a - k, not k - a");
    }

    // sub i16 reg-reg with borrow: a=0x0105, b=0x0007 -> d = 0x00FE.
    // MOVF b_lo(0x0822) SUBWF a_lo(0x0220) MOVWF d_lo(0x00A4)
    // MOVF b_hi(0x0823) BTFSS STATUS,0(0x1C03) ADDLW 1(0x3E01)
    // SUBWF a_hi(0x0221) MOVWF d_hi(0x00A5)
    {
        let mut p = Pic14::new(vec![0x0822, 0x0220, 0x00A4, 0x0823, 0x1C03, 0x3E01, 0x0221, 0x00A5]);
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
        let mut p = Pic14::new(vec![0x3007, 0x0220, 0x00A4, 0x3000, 0x1C03, 0x3E01, 0x0221, 0x00A5]);
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    // %1=0x25, %c=0x26, scratch=0x70.
    assert!(asm.contains("MOVF 0x25, W"), "load a:\n{asm}");
    assert!(asm.contains("XORLW 0x01"), "xor with const b:\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "store xor to scratch:\n{asm}");
    assert!(asm.contains("BTFSS STATUS, 2"), "ne tests Z inverted (BTFSS):\n{asm}");
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
    let asm = select(&m, &addrs);
    // %a=0x29/2A, %b=0x2B/2C, %c=0x2D.
    assert!(
        asm.contains("MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVF 0x2C, W\n    BTFSS STATUS, 0 ; C\n    ADDLW 0x01\n    SUBWF 0x2A, W"),
        "i16 borrow chain:\n{asm}"
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
    let asm = select(&m, &addrs);
    // Chain first (C), then the eq accumulation (Z = a == b), then
    // C && !Z. %a=0x29/2A, %b=0x2B/2C, scratch=0x70.
    assert!(
        asm.contains("MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVF 0x2C, W\n    BTFSS STATUS, 0 ; C\n    ADDLW 0x01\n    SUBWF 0x2A, W\n    MOVF 0x29, W\n    XORWF 0x2B, W\n    MOVWF 0x70\n    MOVF 0x2A, W\n    XORWF 0x2C, W\n    IORWF 0x70, W\n    MOVWF 0x70"),
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
    let asm = select(&m, &addrs);
    // %a=0x29/2A, %b=0x2B/2C, scratch=0x70.
    assert!(
        asm.contains("MOVLW 0x80\n    XORWF 0x2A, W\n    MOVWF 0x70\n    MOVF 0x2B, W\n    SUBWF 0x29, W\n    MOVLW 0x80\n    XORWF 0x2C, W\n    BTFSS STATUS, 0 ; C\n    ADDLW 0x01\n    SUBWF 0x70, W"),
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
    let asm = select(&m, &addrs);
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
    // Unsigned i16 chain: MOVF b_lo(0x0822) SUBWF a_lo(0x0220)
    // MOVF b_hi(0x0823) BTFSS C(0x1C03) ADDLW 1(0x3E01) SUBWF a_hi(0x0221).
    let u16 = vec![0x0822, 0x0220, 0x0823, 0x1C03, 0x3E01, 0x0221];
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
    // Signed i16 chain: MOVLW 0x80(0x3080) XORWF a_hi(0x0621) MOVWF scratch
    // MOVF b_lo SUBWF a_lo MOVLW 0x80 XORWF b_hi(0x0623) BTFSS C ADDLW 1
    // SUBWF scratch,W(0x0270).
    let slt16 = {
        let mut v = vec![0x3080, 0x0621, 0x00F0, 0x0822, 0x0220, 0x3080, 0x0623, 0x1C03, 0x3E01, 0x0270];
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
    let asm = select(&m, &addrs(map));
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
        assert_eq!(got, want, "assembled {pred}16(0x{xhi:02X}{xlo:02X},0x{yhi:02X}{ylo:02X}) must be {want}");
    }
}
