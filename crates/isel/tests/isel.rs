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
    // RAM indirect load (%w = load i8 %p): IRP cleared first (bank-0 base
    // 0x21 — a prior bank-2/3 access would leave IRP=1), then
    // W = %i; W += 0x21 (base_lo); FSR = W; W = INDF; %w = W.
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVF 0x29, W\n    ADDLW 0x21\n    MOVWF FSR\n    MOVF INDF, W"),
        "IRP cleared + FSR = base_lo + i for @ram:\n{asm}"
    );
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
    // M10: every reader sets PCLATH to the table's 256-byte window before
    // the computed PCL jump (fixes the latent window bug — a table past
    // 0x100 needs PCLATH != 0 to land the jump).
    assert!(asm.contains("MOVLW HIGH(table)"), "reader must set PCLATH:\n{asm}");
    assert!(asm.contains("MOVWF PCLATH"), "reader must write PCLATH:\n{asm}");
    assert!(
        asm.find("MOVLW HIGH(table)").unwrap() < asm.find("ADDLW LOW(table)").unwrap(),
        "PCLATH must be set before the computed jump:\n{asm}"
    );
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs(&map));
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
    assert_eq!(sim_run(ir, &map, &seed(2, 2), 0x22), 0x17, "a[1+2+4] with i=2, j=2");
    assert_eq!(sim_run(ir, &map, &seed(3, 1), 0x22), 0x16, "a[1+3+2] with i=3, j=1");
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
    let asm = select(&m, &addrs);
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
        let asm = select(&m, &addrs);
        assert!(
            asm.contains(&format!("{irp_line}\n    MOVF 0x29, W\n    {lit}\n    MOVWF FSR")),
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
    let addrs = addrs(&[("in", 0x24), ("g", 0x78), ("main::i", 0x29), ("main::v", 0x2A)]);
    let _ = select(&m, &addrs);
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
    let addrs = addrs(&[("in", 0x24), ("g", 0x150), ("main::i", 0x29), ("main::v", 0x2A)]);
    let _ = select(&m, &addrs);
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
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("f::i", 0x25), ("f::0", 0x120), ("f::v", 0x29)]);
    let asm = select(&m, &addrs);
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
    // The chain folds k = 1 + 1 = 2 into the fast path's ADDLW literal;
    // the IRP clear (bank-0 base) precedes the FSR setup.
    assert!(
        asm.contains("BCF STATUS, 7\n    MOVF 0x29, W\n    ADDLW 0x23\n    MOVWF FSR\n    MOVF INDF, W"),
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
    let asm = select(&m, &addrs);
    // The size-byte copy: alloca bytes 0x25..0x28 -> param slot 0x2B..0x2E.
    for i in 0..4u16 {
        assert!(
            asm.contains(&format!("MOVF 0x{:02X}, W\n    MOVWF 0x{:02X}", 0x25 + i, 0x2B + i)),
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
    let asm = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
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
    let _ = select(&m, &addrs);
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
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs(&map));
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
    let asm = select(&m, &addrs(&map));
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
        let asm = select(&m, &addrs(&map));
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
    let asm = select(&m, &addrs(&map));
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x2B"),
        "copy @g byte 0 into sum::p:\n{asm}"
    );
    let seed = [(0x20u16, 3u8), (0x21, 0x00), (0x22, 0x34), (0x23, 0x12)];
    assert_eq!(sim_run(ir, &map, &seed, 0x24), 0x37, "sum(g) with g = {{3, 0x1234}}");
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
    let asm = select(&m, &addrs(&map_refs(map)));
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
                "RLF 0x30, F",   // num <<= 1 (dividend param = quotient accumulator)
                "SUBWF 0x32, F", // rem_lo = __scr+0
                "ADDLW 0x01",    // borrow fold
                "SUBWF 0x33, F", // rem_hi = __scr+1
                "BSF 0x30, 0",   // quotient bit into num
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
                "RLF 0x40, F",   // num_lo <<= 1
                "SUBWF 0x44, F", // rem_lo = __scr+0
                "INCFSZ 0x43, W", // den_hi + borrow: the borrow idiom
                "SUBWF 0x45, F", // rem_hi = __scr+1
                "BSF 0x40, 0",   // quotient bit
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
                "BSF 0x44, 1",   // flags = __scr+0
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
        let asm = select(&parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        assert!(
            asm.contains(&format!("    CALL {name}")),
            "{name} call:\n{asm}"
        );
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body.split("main:").next().expect("main label after routine");
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
        let (ina, inb, out) = if wide { (0x20, 0x22, 0x24) } else { (0x20, 0x21, 0x22) };
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

/// A routine slot that the banking pass would relocate (0xA0-0xEF is bank
/// 1) would need BANKSELs inserted inside the skip-sensitive recipe loops —
/// loud assert, never a silent miscompile. 0xA0 slid under the old ≤0xFF
/// bound even though the asm encoder rejects file registers past 0x7F.
#[test]
#[should_panic(expected = "bank-0")]
fn panics_on_banked_routine_slot() {
    let (ir, mut map) = routine_module("__mul_u8");
    for (k, v) in map.iter_mut() {
        if k == "__mul_u8::__scr" {
            *v = 0xA0; // bank 1 (0x80-0xEF): pre-fix this passed silently
        }
    }
    let _ = select(&parse(&ir), &addrs(&map_refs(&map)));
}

/// A routine slot past the 0x7F bound entirely (beyond RAM) must also fail
/// loudly.
#[test]
#[should_panic(expected = "bank-0")]
fn panics_on_routine_slot_past_ram() {
    let (ir, mut map) = routine_module("__mul_u8");
    for (k, v) in map.iter_mut() {
        if k == "__mul_u8::__scr" {
            *v = 0x120;
        }
    }
    let _ = select(&parse(&ir), &addrs(&map_refs(&map)));
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
    let asm = select(&parse(&shift_module("shl", "i16", "3")), &addrs(&map_refs(&shift_map16())));
    assert_eq!(asm.matches("    BCF STATUS, 0").count(), 3, "one BCF per step:\n{asm}");
    assert_eq!(asm.matches("    RLF 0x27, F").count(), 3, "lo byte rotated each step:\n{asm}");
    assert_eq!(asm.matches("    RLF 0x28, F").count(), 3, "hi byte rotated each step:\n{asm}");
    assert!(!asm.contains("RRF"), "shl must not emit rrf:\n{asm}");

    // lshr i16 %a, 2 -> 2 x (BCF C / RRF hi / RRF lo): the high byte MUST
    // rotate before the low byte, or the shifted-out bit lands in the wrong
    // place.
    let asm = select(&parse(&shift_module("lshr", "i16", "2")), &addrs(&map_refs(&shift_map16())));
    assert_eq!(asm.matches("    BCF STATUS, 0").count(), 2, "one BCF per step:\n{asm}");
    assert_eq!(asm.matches("    RRF 0x28, F").count(), 2, "hi byte first:\n{asm}");
    assert_eq!(asm.matches("    RRF 0x27, F").count(), 2, "lo byte second:\n{asm}");
    let hi = asm.find("    RRF 0x28, F").expect("hi rrf");
    let lo = asm.find("    RRF 0x27, F").expect("lo rrf");
    assert!(hi < lo, "lshr must shift the high byte first:\n{asm}");
    assert!(!asm.contains("RLF"), "lshr must not emit rlf:\n{asm}");

    // ashr i8 %a, 2 -> C set from the sign bit (BTFSC/BSF + BTFSS/BCF) before
    // each RRF; the rrf chain is a single byte for i8 (dst = main::s = 0x26).
    let asm = select(&parse(&shift_module("ashr", "i8", "2")), &addrs(&map_refs(&shift_map8())));
    assert_eq!(asm.matches("    RRF 0x26, F").count(), 2, "one rrf per step:\n{asm}");
    assert_eq!(asm.matches("    RRF").count(), 2, "i8 ashr must have no second byte:\n{asm}");
    let btfsc = asm.find("    BTFSC 0x26, 7").expect("sign-bit test");
    let btfss = asm.find("    BTFSS 0x26, 7").expect("sign-bit test 2");
    let rrf = asm.find("    RRF 0x26, F").expect("rrf");
    assert!(
        btfsc < btfss && btfss < rrf,
        "C must be set from the sign bit before each rrf:\n{asm}"
    );

    // shl i16 %a, 0 -> a plain copy (MOVF/MOVWF pairs), no rotation at all.
    let asm = select(&parse(&shift_module("shl", "i16", "0")), &addrs(&map_refs(&shift_map16())));
    assert!(!asm.contains("RLF") && !asm.contains("RRF"), "k=0 must be a plain copy:\n{asm}");
    assert!(asm.contains("    MOVF 0x25, W"), "copy lo:\n{asm}");
    assert!(asm.contains("    MOVWF 0x27"), "store lo:\n{asm}");
    assert!(asm.contains("    MOVF 0x26, W"), "copy hi:\n{asm}");
    assert!(asm.contains("    MOVWF 0x28"), "store hi:\n{asm}");

    // i8 single-byte chains: shl i8 %a, 1 -> one RLF on the only byte;
    // lshr i8 %a, 1 -> one RRF on the only byte.
    let asm = select(&parse(&shift_module("shl", "i8", "1")), &addrs(&map_refs(&shift_map8())));
    assert_eq!(asm.matches("    RLF 0x26, F").count(), 1, "i8 shl is one byte:\n{asm}");
    assert_eq!(asm.matches("    RLF").count(), 1, "i8 shl must have no second byte:\n{asm}");
    let asm = select(&parse(&shift_module("lshr", "i8", "1")), &addrs(&map_refs(&shift_map8())));
    assert_eq!(asm.matches("    RRF 0x26, F").count(), 1, "i8 lshr is one byte:\n{asm}");
    assert_eq!(asm.matches("    RRF").count(), 1, "i8 lshr must have no second byte:\n{asm}");
}

/// k >= width is LLVM poison: the result is defined as no value, so a loud
/// panic beats emitting a wrong-but-deterministic result.
#[test]
#[should_panic(expected = "const shift count 16 out of range")]
fn panics_on_inline_shift_count_ge_width_i16() {
    let m = parse(&shift_module("shl", "i16", "16"));
    select(&m, &addrs(&map_refs(&shift_map16())));
}

#[test]
#[should_panic(expected = "const shift count 8 out of range")]
fn panics_on_inline_shift_count_ge_width_i8() {
    let m = parse(&shift_module("lshr", "i8", "8"));
    select(&m, &addrs(&map_refs(&shift_map8())));
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
                "ANDLW 0x07",     // count & (8-1)
                "MOVWF 0x32",     // __scr::cnt@0 = masked count
                "MOVF 0x32, F",   // zero test
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
                "CLRF 0x45",  // high byte of the masked count
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
        let asm = select(&parse(&ir), &addrs(&map_refs(&map)));
        assert!(asm.contains(&format!("{name}:")), "{name} label:\n{asm}");
        assert!(
            asm.contains(&format!("    CALL {name}")),
            "{name} call:\n{asm}"
        );
        let start = asm.find(&format!("{name}:")).expect("routine label");
        let body = &asm[start..];
        let body = body.split("main:").next().expect("main label after routine");
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
        let (ina, inb, out) = if wide { (0x20, 0x22, 0x24) } else { (0x20, 0x21, 0x22) };
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

/// A shift-routine slot that the banking pass would relocate (bank 1) would
/// need BANKSELs inside the skip-sensitive loop — loud assert, same as the
/// mul/div/rem recipes.
#[test]
#[should_panic(expected = "bank-0")]
fn panics_on_banked_shift_routine_slot() {
    let (ir, mut map) = routine_module("__shl_u16");
    for (k, v) in map.iter_mut() {
        if k == "__shl_u16::__scr" {
            *v = 0xA0; // bank 1 (0x80-0xEF)
        }
    }
    let _ = select(&parse(&ir), &addrs(&map_refs(&map)));
}

// ---- Milestone 10: const-table PCLATH readers and the page-0 bound ----

/// A const table of `size` bytes: bytes 0..min(size,256) = 0x00..0xFF,
/// bytes 256+n = 0x11+n — distinctive per-byte values, so a wrong
/// chunk/window lands on the wrong RETLW (a readable wrong-answer, not a
/// crash).
fn const_table_global(name: &str, size: usize) -> ir::Global {
    let bytes: Vec<u8> = (0..size)
        .map(|i| if i < 256 { i as u8 } else { 0x11 + (i - 256) as u8 })
        .collect();
    ir::Global {
        name: name.into(),
        ty: ir::Ty::I8,
        is_const: true,
        size: size as u16,
        bytes,
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
            },
            const_table_global("t", 300),
        ],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22), ("main::i", 0x24), ("main::v", 0x26)]);
    let asm = select(&m, &addrs);
    // Caller: lo-temp 0x71, hi-temp 0x70, chunk-bit test, both entry CALLs.
    assert!(asm.contains("MOVWF 0x71"), "lo index temp (retval_lo):\n{asm}");
    assert!(asm.contains("MOVWF 0x70"), "hi temp / chunk bit (scratch):\n{asm}");
    assert!(asm.contains("BTFSC 0x70, 0"), "chunk-bit test:\n{asm}");
    assert!(asm.contains("    CALL __read_t\n"), "chunk-0 entry call:\n{asm}");
    assert!(asm.contains("    CALL __read_t_hi"), "chunk-1 entry call:\n{asm}");
    assert!(asm.contains("GOTO tmp"), "fresh .hi/.done labels:\n{asm}");
    // Reader entry 0: PCLATH = window of t, computed jump into t. The
    // index (W) is stashed in the fixed scratch byte across the PCLATH set
    // (MOVLW HIGH would clobber it otherwise).
    assert!(asm.contains("__read_t:\n    MOVWF 0x70"), "chunk-0 reader stashes the index:\n{asm}");
    assert!(asm.contains("MOVLW HIGH(t)"), "chunk-0 PCLATH set:\n{asm}");
    assert!(asm.contains("ADDLW LOW(t)"), "chunk-0 index add:\n{asm}");
    // Reader entry 1: window of the fresh `t_1` chunk label (t + 256).
    assert!(asm.contains("__read_t_hi:\n    MOVWF 0x70"), "chunk-1 reader stashes the index:\n{asm}");
    assert!(asm.contains("MOVLW HIGH(t_1)"), "chunk-1 PCLATH set:\n{asm}");
    assert!(asm.contains("ADDLW LOW(t_1)"), "chunk-1 index add:\n{asm}");
    // Window-fit directives: `.align 256` 256-aligns the chunk-0 base and
    // `.table t 300` lets the assembler enforce the window fit loudly.
    assert!(asm.contains("    .align 256"), "chunked base must be aligned:\n{asm}");
    assert!(asm.contains("    .table t 300"), "window-fit directive before the base label:\n{asm}");
    // Exactly size RETLWs, split 256 + (size-256) across the two chunks,
    // chunk 1 IMMEDIATELY after chunk 0 (no reader entry between — the
    // chunk-1 reader comes after the whole table).
    assert_eq!(asm.matches("RETLW").count(), 300, "one RETLW per byte:\n{asm}");
    let t = asm.find("\nt:").unwrap();
    let t1 = asm.find("\nt_1:").unwrap();
    let hi = asm.find("__read_t_hi:").unwrap();
    let chunk0 = &asm[t..t1];
    assert_eq!(chunk0.matches("RETLW").count(), 256, "chunk 0 = 256 bytes:\n{asm}");
    let chunk1 = &asm[t1..hi];
    assert_eq!(chunk1.matches("RETLW").count(), 44, "chunk 1 = size-256 bytes:\n{asm}");
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
            },
            ir::Global {
                name: "j".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
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
    let _ = select(&m, &addrs);
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
    let _ = select(&m, &addrs);
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
            },
            const_table_global("t", 300),
            const_table_global("t_1", 1),
        ],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22), ("main::i", 0x24), ("main::v", 0x26)]);
    let _ = select(&m, &addrs);
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
            },
            const_table_global("t", 300),
            const_table_global("__read_t_hi", 1),
        ],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22), ("main::i", 0x24), ("main::v", 0x26)]);
    let _ = select(&m, &addrs);
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
            },
            const_table_global("aaa_fill", 231),
            ir::Global {
                name: "table".into(),
                ty: ir::Ty::I8,
                is_const: true,
                size: 4,
                bytes: vec![10, 20, 30, 40],
                addr: None,
            },
        ],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::i", 0x25), ("main::v", 0x26)]);
    let asm = select(&m, &addrs);
    // Load-bearing precondition: the table lands in a NONZERO 256-byte
    // window and fits it (LOW + 4 <= 0x100) — a reader without the PCLATH
    // set would jump into window 0 and return the wrong byte.
    let base = label_addr(&asm, "table");
    assert!(
        base >= 0x100,
        "table must sit past 0x100 for the PCLATH set to be load-bearing (base 0x{base:03X}):\n{asm}"
    );
    assert!(base & 0xFF <= 0xFC, "table must fit its window (base 0x{base:03X}):\n{asm}");
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
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
    let map = [("in", 0x20u16), ("out", 0x22), ("main::i", 0x24), ("main::v", 0x26)];
    let asm0 = select(&module_with_globals(&ir(0), globals()), &addrs(&map));
    let base = label_addr(&asm0, "t");
    assert!(
        base & 0xFF == 0,
        "chunk 0 must start 256-aligned for the computed jumps to cover all 300 bytes (base 0x{base:03X}):\n{asm0}"
    );
    // (in, k, expected byte)
    let cases: &[(u16, u8, u8)] = &[
        (2, 0, 0x02),     // chunk 0
        (256, 0, 0x11),   // chunk-1 first byte
        (299, 0, 0x3C),   // chunk-1 last byte (0x11 + 43)
        (290, 0, 0x33),   // chunk-1 (0x11 + 34)
        (0xF0, 0x20, 0x21), // lo 0xF0 + k 0x20 = 0x110 -> in-chunk 0x10, hi 1 -> table[272] = 0x11 + 16
    ];
    for (in_val, k, want) in cases {
        let m = module_with_globals(&ir(*k), globals());
        let asm = select(&m, &addrs(&map));
        let got = sim_run_asm(&asm, &[(0x20, *in_val as u8), (0x21, (*in_val >> 8) as u8)], 0x22);
        assert_eq!(got, *want, "table[{in_val}] with k 0x{k:02X} must read 0x{want:02X}:\n{asm}");
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
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
            },
            const_table_global("t", 256),
        ],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22), ("main::i", 0x24), ("main::v", 0x26)]);
    let asm = select(&m, &addrs);
    // Chunked shape: `.align 256`, `.table t 256`, the `t_1` chunk label,
    // and the `__read_t_hi` entry — a 256-byte table is two chunks, not one.
    assert!(
        asm.contains("    .align 256"),
        "256-byte table must take the chunked branch:\n{asm}"
    );
    assert!(asm.contains("    .table t 256"), "window-fit directive with the full size:\n{asm}");
    assert!(asm.contains("\nt_1:"), "empty chunk-1 label must be emitted:\n{asm}");
    assert!(asm.contains("__read_t_hi:"), "chunk-1 reader entry must be emitted:\n{asm}");
    // 256 RETLWs total, all in chunk 0; chunk 1 is empty (t_1 == t + 256,
    // __read_t_hi immediately after the label).
    assert_eq!(asm.matches("RETLW").count(), 256, "one RETLW per byte:\n{asm}");
    let t = asm.find("\nt:").unwrap();
    let t1 = asm.find("\nt_1:").unwrap();
    let hi = asm.find("__read_t_hi:").unwrap();
    assert_eq!(&asm[t..t1].matches("RETLW").count(), &256, "chunk 0 = 256 bytes:\n{asm}");
    assert_eq!(&asm[t1..hi].matches("RETLW").count(), &0, "chunk 1 = 0 bytes:\n{asm}");
    let base = label_addr(&asm, "t");
    assert_eq!(label_addr(&asm, "t_1"), base + 256, "t_1 = t + 256:\n{asm}");
    assert_eq!(base & 0xFF, 0, "chunk-0 base must be 256-aligned:\n{asm}");
    // And it assembles + simulates. The table's natural base (goto + main +
    // reader) is NOT 256-aligned here, so the old single-entry cut would
    // fail assembly with the `.table` window assert — the fix must make the
    // layout alignment irrelevant. Reads land in chunk 0 only (0..255).
    assert_eq!(sim_run_asm(&asm, &[(0x20, 0x00), (0x21, 0x00)], 0x22), 0x00, "table[0] = 0x00:\n{asm}");
    assert_eq!(sim_run_asm(&asm, &[(0x20, 0xFF), (0x21, 0x00)], 0x22), 0xFF, "table[255] = 0xFF:\n{asm}");
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
fn call_emits_pclath_set_and_restore() {
    // main calls helper: the emitted asm must wrap the CALL in
    // `MOVLW PAGE(helper); MOVWF PCLATH; CALL helper; MOVLW PAGE(main);
    // MOVWF PCLATH` — the restore literal is the CALLER's page (its
    // intra-function GOTOs run with it). `__start` sets PAGE(main) before
    // CALL main and omits the restore (the program ends with SLEEP).
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
    let asm = select(&m, &addrs);
    assert!(
        asm.contains("MOVLW PAGE(helper)\n    MOVWF PCLATH\n    CALL helper\n    MOVLW PAGE(main)\n    MOVWF PCLATH"),
        "set/CALL/restore pairs around CALL helper:\n{asm}"
    );
    // __start: PAGE(main) set before CALL main, then SLEEP — no restore.
    assert!(
        asm.contains("__start:\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n    CALL main\n    SLEEP"),
        "__start PCLATH set with no restore:\n{asm}"
    );
}

#[test]
fn const_read_emits_pclath_discipline() {
    // A const-table read (`CALL __read_t`) gets the same discipline: the
    // caller sets PAGE(__read_t) before the CALL and restores PAGE(main)
    // right after (the returned byte survives via the fixed scratch byte).
    let m = module_with_globals(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n\
           store i8 %v @out\n    ret void\n",
        vec![const_table_global("t", 4)],
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::i", 0x25), ("main::v", 0x26)]);
    let asm = select(&m, &addrs);
    // The set goes before the index computation (W = index is the reader's
    // input — the set's MOVLW must not clobber it); the restore preserves
    // the returned byte in scratch (0x70) across its own MOVLW.
    assert!(
        asm.contains("MOVLW PAGE(__read_t)\n    MOVWF PCLATH"),
        "set before CALL __read_t:\n{asm}"
    );
    assert!(
        asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVLW PAGE(main)\n    MOVWF PCLATH\n    MOVF 0x70, W"),
        "restore after CALL __read_t, byte preserved:\n{asm}"
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
    let addrs = addrs(&[("in", 0x20), ("out", 0x21), ("main::1", 0x25), ("main::a", 0x26)]);
    let _ = select(&m, &addrs);
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
    let asm = select(&m, &addrs);
    assert!(asm.contains("    org 0x0800"), ".org 0x800 before helper:\n{asm}");
    assert_eq!(label_addr(&asm, "helper"), 0x800, "helper in page 1:\n{asm}");
    assert!(label_addr(&asm, "main") < 0x800, "main stays in page 0:\n{asm}");
}

#[test]
fn multi_page_module_runs_in_sim() {
    // M11 load-bearing SIM: main (padded to fill page 0) calls helper which
    // the greedy assignment moves to page 1 via `.org 0x800`; the CALL is
    // cross-page, so both halves of the discipline are exercised — helper's
    // intra-function GOTO proves the SET (PCLATH = PAGE(helper) on entry)
    // and main's post-call GOTO proves the RESTORE (PCLATH back to
    // PAGE(main)). A const table lands in page 1 too: its `CALL __read_t`
    // gets PAGE(__read_t) and the reader's computed goto crosses into the
    // table's window. helper(x) = x == 0 ? 100 : x; main: r = helper(in);
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
             block end:\n    %p = phi i8 %v then %x else\n    ret i8 %p\n",
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
    ]);
    let asm = select(&m, &addrs);
    // Load-bearing preconditions: helper and the table land in page 1.
    assert!(asm.contains("    org 0x0800"), ".org 0x800 must be emitted:\n{asm}");
    assert_eq!(label_addr(&asm, "helper"), 0x800, "helper must land in page 1:\n{asm}");
    let t = label_addr(&asm, "t");
    assert!(t >= 0x800 && t < 0x1000, "table must land in page 1 (base 0x{t:03X}):\n{asm}");
    // Hand-computed results (see the doc comment).
    assert_eq!(sim_run_asm(&asm, &[(0x20, 0)], 0x21), 100, "in=0: helper=100, t[0]=0:\n{asm}");
    assert_eq!(sim_run_asm(&asm, &[(0x20, 1)], 0x21), 2, "in=1: helper=1, t[1]=1:\n{asm}");
    assert_eq!(sim_run_asm(&asm, &[(0x20, 2)], 0x21), 4, "in=2: helper=2, t[2]=2:\n{asm}");
    assert_eq!(sim_run_asm(&asm, &[(0x20, 3)], 0x21), 6, "in=3: helper=3, t[3]=3:\n{asm}");
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
    ir.push_str(&format!("fn f4(void) ()\n  block entry:\n{}    ret void\n", pad_body(10)));
    let m = parse(&ir);
    let mut pairs: Vec<(String, u16)> = vec![("in".to_string(), 0x20)];
    for i in 0..4 {
        pairs.push((format!("f{i}::a"), 0x25));
    }
    pairs.push(("f4::a".to_string(), 0x25));
    let refs: Vec<(&str, u16)> = pairs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let _ = select(&m, &addrs(&refs));
}
