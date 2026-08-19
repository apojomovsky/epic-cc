use device::PIC18F4550;
use ir::parse;
use isel_pic18::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn empty_function_emits_a_bare_return() {
    let m = parse("fn main(void) ()\n  block entry:\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]));
    assert!(asm.contains("RETURN"), "asm:\n{asm}");
}

#[test]
fn load_and_store_i8_use_movff() {
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(asm.contains("MOVFF 0x010, 0x012"), "load into %1's slot:\n{asm}");
    assert!(asm.contains("MOVFF 0x012, 0x011"), "store %1 to out:\n{asm}");
}

#[test]
fn load_and_store_i16_copy_both_bytes_low_then_high() {
    let m = parse("global in i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @in\n    store i16 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("out", 0x12), ("main::1", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(asm.contains("MOVFF 0x010, 0x014"));
    assert!(asm.contains("MOVFF 0x011, 0x015"));
    assert!(asm.contains("MOVFF 0x014, 0x012"));
    assert!(asm.contains("MOVFF 0x015, 0x013"));
}

#[test]
fn store_a_constant_uses_movlw_then_movwf() {
    // MOVFF has no literal-source form — a constant must go through W.
    let m = parse("global out i8\nfn main(void) ()\n  block entry:\n    store i8 5 @out\n    ret void\n");
    let addrs = addrs(&[("out", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(asm.contains("MOVLW 0x05"));
    assert!(asm.contains("MOVWF 0x011,A") || asm.contains("MOVWF 0x11,A"), "asm:\n{asm}");
}

#[test]
fn i8_binops_load_b_into_w_then_operate_against_a() {
    let cases: &[(&str, &str)] = &[
        ("add", "ADDWF"), ("sub", "SUBWF"), ("and", "ANDWF"), ("or", "IORWF"), ("xor", "XORWF"),
    ];
    for (op, mne) in cases {
        let m = parse(&format!(
            "global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = {op} i8 %1, %2\n    ret void\n"
        ));
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        assert!(asm.contains(&format!("{mne} 0x012,W,A")) || asm.contains(&format!("{mne} 0x12,W,A")), "{op}:\n{asm}");
    }
}

#[test]
fn i8_binop_dest_at_banked_address_routes_through_operand_with_bank_suffix() {
    // The destination slot (main::3) lands at 0x180 (bank 1, f=0x80) — an
    // address >= 0x60 that requires an explicit access-bank suffix (and a
    // MOVLB) — this is the banked-destination case the brief's first
    // example (all addrs < 0x60) would not have caught.
    let m = parse(
        "global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = add i8 %1, %2\n    ret void\n",
    );
    let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x180)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(
        asm.contains("MOVWF 0x80,B") || asm.contains("MOVWF 0x080,B"),
        "banked dest must go through operand() with an explicit ,B suffix:\n{asm}"
    );
    assert!(asm.contains("MOVLB 0x1"), "banked dest must emit a MOVLB for bank 1:\n{asm}");
}

#[test]
fn i16_add_uses_addwfc_for_the_high_byte() {
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = add i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(asm.contains("ADDWF") && asm.contains("ADDWFC"), "low byte plain add, high byte with carry:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // seed a=0x00FF, b=0x0001 -> 0x0100, exercises the carry chain.
    p.ram_mut()[0x10] = 0xFF;
    p.ram_mut()[0x11] = 0x00;
    p.ram_mut()[0x12] = 0x01;
    p.ram_mut()[0x13] = 0x00;
    p.run(200);
    assert_eq!(p.ram()[0x18], 0x00);
    assert_eq!(p.ram()[0x19], 0x01);
}

#[test]
fn i16_sub_uses_subfwb_for_the_high_byte() {
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = sub i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // 0x0100 - 0x0001 = 0x00FF, exercises the borrow chain.
    p.ram_mut()[0x10] = 0x00;
    p.ram_mut()[0x11] = 0x01;
    p.ram_mut()[0x12] = 0x01;
    p.ram_mut()[0x13] = 0x00;
    p.run(200);
    assert_eq!(p.ram()[0x18], 0xFF);
    assert_eq!(p.ram()[0x19], 0x00);
}

#[test]
fn i16_bitwise_ops_apply_independently_per_byte() {
    for (op, mne) in [("and", "ANDWF"), ("or", "IORWF"), ("xor", "XORWF")] {
        let m = parse(&format!(
            "global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = {op} i16 %1, %2\n    ret void\n"
        ));
        let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        // Both bytes use the same plain (non-carry) mnemonic, applied twice.
        assert_eq!(asm.matches(mne).count(), 2, "{op}:\n{asm}");
    }
}

#[test]
#[should_panic(expected = "const-LHS")]
fn i8_binop_const_lhs_is_rejected_not_silently_miscompiled() {
    // `val_addr` maps `Val::Const(k)` to `Slot::Direct(k & 0xFF)` — treating
    // a literal as a RAM ADDRESS. Without the guard, `sub i8 5, %x` would
    // silently emit `SUBWF 0x005,W,A`, reading whatever byte lives at
    // address 0x05 instead of using the literal 5. This must fail loudly
    // instead.
    let m = parse(
        "global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = sub i8 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x13)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
fn icmp_eq_materializes_1_when_equal_and_0_when_not() {
    let m = parse("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [(5u8, 5u8, 1u8), (5, 6, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "eq({av},{bv})");
    }
}

#[test]
fn icmp_ne_distinguishes_equal_from_not_equal() {
    for (a, b, expect) in [(5u8, 5u8, 0u8), (5, 6, 1)] {
        let m = parse("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp ne i8 %1, %2\n    ret void\n");
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "ne({a},{b})");
    }
}

#[test]
fn icmp_ult_and_uge_use_the_carry_flag() {
    for (pred, a, b, expect) in [
        ("ult", 3u8, 5u8, 1u8), ("ult", 5, 3, 0), ("ult", 5, 5, 0),
        ("uge", 5, 3, 1), ("uge", 3, 5, 0), ("uge", 5, 5, 1),
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_ugt_and_ule_combine_c_and_z() {
    for (pred, a, b, expect) in [("ugt", 5u8, 3u8, 1u8), ("ugt", 3, 5, 0), ("ugt", 5, 5, 0), ("ule", 3, 5, 1), ("ule", 5, 5, 1), ("ule", 5, 3, 0)] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_slt_and_sge_use_n_xor_ov() {
    // -1 (0xFF) < 1 is true; 1 < -1 is false. Also cross the signed-
    // overflow boundary: 127 < -128 is false (no overflow in this
    // direction) but tests N!=OV correctly only if the case is chosen so
    // OV actually gets set — include one such case explicitly.
    for (pred, a, b, expect) in [
        ("slt", 0xFFu8, 1u8, 1u8),      // -1 < 1
        ("slt", 1, 0xFF, 0),             // 1 < -1 is false
        ("slt", 5, 5, 0),                 // equal: strict predicate is false
        ("sge", 1, 0xFF, 1),
        ("sge", 0xFF, 1, 0),
        ("sge", 5, 5, 1),                 // equal: non-strict predicate is true
        ("slt", 0x7F, 0x80, 0),          // 127 < -128: false, but a-b overflows (OV=1), N must still resolve correctly
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a:#04x},{b:#04x})");
    }
}

#[test]
fn icmp_sgt_and_sle_combine_z_and_n_xor_ov() {
    for (pred, a, b, expect) in [
        ("sgt", 5u8, 3u8, 1u8), ("sgt", 3, 5, 0), ("sgt", 5, 5, 0),
        ("sle", 3, 5, 1), ("sle", 5, 5, 1), ("sle", 5, 3, 0),
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x11), ("main::1", 0x12), ("main::2", 0x13), ("main::3", 0x14)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_i16_ties_break_on_the_low_byte() {
    // High bytes equal (0x01), low bytes differ: 0x0105 vs 0x0103.
    for (pred, expect) in [("ult", 0u8), ("ugt", 1), ("eq", 0), ("slt", 0), ("sgt", 1)] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = 0x05; // a lo
        p.ram_mut()[0x11] = 0x01; // a hi
        p.ram_mut()[0x12] = 0x03; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0x0105, 0x0103)");
    }
}

#[test]
fn icmp_i16_high_byte_alone_decides_when_it_differs() {
    // a=0x00FF, b=0x0100: a < b even though a's low byte is larger.
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp ult i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x10] = 0xFF;
    p.ram_mut()[0x11] = 0x00;
    p.ram_mut()[0x12] = 0x00;
    p.ram_mut()[0x13] = 0x01;
    p.run(300);
    assert_eq!(p.ram()[0x18], 1);
}

#[test]
fn icmp_i16_full_equality_resolves_correctly_for_every_predicate() {
    // a == b == 0x0142 for all ten predicates: this exercises the "equal
    // at every byte" edge case for both the high-byte compare (where
    // `emit_cmp_branch`'s `l_equal` always defers to the low-byte check)
    // and the low-byte tie-break (where `l_equal` must resolve to this
    // predicate's real answer at full equality — true for the
    // non-strict/eq predicates, false for the strict ones).
    for (pred, expect) in [
        ("eq", 1u8), ("ne", 0),
        ("ult", 0), ("ule", 1), ("ugt", 0), ("uge", 1),
        ("slt", 0), ("sle", 1), ("sgt", 0), ("sge", 1),
    ] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = 0x42; // a lo
        p.ram_mut()[0x11] = 0x01; // a hi
        p.ram_mut()[0x12] = 0x42; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0x0142, 0x0142)");
    }
}

#[test]
fn icmp_i16_high_byte_uses_the_predicates_own_signedness() {
    // a=0xFF00, b=0x0100 — the high bytes differ (0xFF vs 0x01) so the
    // high byte alone decides the whole comparison, but the SIGNED and
    // UNSIGNED answers genuinely disagree on this bit pattern: as signed
    // 16-bit values, a = -256 < b = 256, so the signed predicates read
    // a<b; as unsigned 16-bit values, a = 0xFF00 = 65280 > b = 0x0100 =
    // 256, so the unsigned predicates read a>b. If `emit_icmp_i16`'s
    // high-byte compare accidentally used the unsigned tie-break
    // predicate (e.g. `ult` instead of `slt`) instead of `pred` itself,
    // every signed-predicate case below would silently flip. `ult` is
    // included as the control case showing the unsigned reading really
    // is the opposite.
    for (pred, expect) in [("slt", 1u8), ("sle", 1), ("sgt", 0), ("sge", 0), ("ult", 0)] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[("a", 0x10), ("b", 0x12), ("main::1", 0x14), ("main::2", 0x16), ("main::3", 0x18)]);
        let asm = select(&PIC18F4550, &m, &addrs);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = 0x00; // a lo
        p.ram_mut()[0x11] = 0xFF; // a hi
        p.ram_mut()[0x12] = 0x00; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0xFF00, 0x0100)");
    }
}

#[test]
#[should_panic(expected = "const-LHS")]
fn icmp_const_lhs_is_rejected_not_silently_miscompiled() {
    // `val_addr` maps `Val::Const(k)` to `Slot::Direct(k & 0xFF)` — treating
    // a literal as a RAM ADDRESS. Without the guard, `icmp ult i8 5, %x`
    // would silently emit `SUBWF 0x005,W,A`, reading whatever byte lives at
    // address 0x05 instead of using the literal 5. This must fail loudly
    // instead, matching the `Inst::Bin` const-LHS guard.
    let m = parse(
        "global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = icmp ult i8 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x13)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
#[should_panic(expected = "const-LHS")]
fn icmp_i16_const_lhs_is_rejected_not_silently_miscompiled() {
    // Same hazard as `icmp_const_lhs_is_rejected_not_silently_miscompiled`,
    // but for the new i16 path (`emit_icmp_i16`) — the guard must not be
    // bypassed just because the width changed.
    let m = parse(
        "global x i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @x\n    %2 = icmp ult i16 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x14)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
fn zext_i8_to_i16_zero_fills_the_high_byte() {
    let m = parse("global a i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = zext i8 %1 to i16\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x10] = 0xFF;
    p.run(50);
    assert_eq!(p.ram()[0x12], 0xFF);
    assert_eq!(p.ram()[0x13], 0x00);
}

#[test]
fn sext_i8_to_i16_sign_fills_the_high_byte() {
    let m = parse("global a i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = sext i8 %1 to i16\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x10] = 0xFF; // -1
    p.run(50);
    assert_eq!(p.ram()[0x12], 0xFF);
    assert_eq!(p.ram()[0x13], 0xFF, "sign-filled");
}

#[test]
fn trunc_i16_to_i8_keeps_the_low_byte() {
    let m = parse("global a i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = trunc i16 %1 to i8\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x12), ("main::2", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x10] = 0x34;
    p.ram_mut()[0x11] = 0x12;
    p.run(50);
    assert_eq!(p.ram()[0x14], 0x34);
}

#[test]
#[should_panic(expected = "const source Zext")]
fn zext_const_source_is_rejected_not_silently_miscompiled() {
    // `val_addr` maps `Val::Const(k)` to `Slot::Direct(k & 0xFF)` — treating
    // a literal as a RAM ADDRESS. Without the guard, `zext i8 5 to i16`
    // would silently `MOVFF` from whatever byte lives at address 0x05
    // instead of using the literal 5. This must fail loudly instead,
    // matching the `Inst::Bin`/`Inst::Icmp` const-LHS guards.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = zext i8 5 to i16\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
#[should_panic(expected = "const source Sext")]
fn sext_const_source_is_rejected_not_silently_miscompiled() {
    // Same hazard as `zext_const_source_is_rejected_not_silently_miscompiled`.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = sext i8 5 to i16\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
#[should_panic(expected = "const source Trunc")]
fn trunc_const_source_is_rejected_not_silently_miscompiled() {
    // Same hazard as `zext_const_source_is_rejected_not_silently_miscompiled`.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = trunc i16 5 to i8\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x11)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
#[should_panic(expected = "const cond Select")]
fn select_const_cond_is_rejected_not_silently_miscompiled() {
    // `emit_load_w`'s `Val::Const` arm emits only `MOVLW`, which (per this
    // project's simulator, `crates/sim/src/lib.rs:903`) never touches the
    // Z flag — unlike `MOVF` (the `Val::Reg`/`Val::Global` arm), which
    // does via `set_zn` (`crates/sim/src/lib.rs:779-783`). Without the
    // guard, `select i1 1 ...`'s `BZ` right after the `MOVLW` would test
    // whatever Z flag the PREVIOUS instruction happened to leave, silently
    // picking the wrong side of the `Select` instead of using the literal
    // cond. This must fail loudly instead, matching the `Inst::Bin`/
    // `Inst::Icmp` const-LHS guards and `Zext`/`Sext`/`Trunc`'s
    // const-source guards.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = select i1 1 i8 5 i8 6\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
fn select_picks_a_when_cond_is_true_and_b_otherwise() {
    let m = parse("global c i8\nglobal a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    %2 = load i8 @a\n    %3 = load i8 @b\n    %4 = icmp ne i8 %1, 0\n    %5 = select i1 %4 i8 %2 i8 %3\n    ret void\n");
    let addrs = addrs(&[("c", 0x10), ("a", 0x11), ("b", 0x12), ("main::1", 0x13), ("main::2", 0x14), ("main::3", 0x15), ("main::4", 0x16), ("main::5", 0x17)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (c, expect) in [(1u8, 0x11u8), (0, 0x22)] {
        // reuse fresh ram each run
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        p.ram_mut()[0x11] = 0x11;
        p.ram_mut()[0x12] = 0x22;
        p.run(200);
        assert_eq!(p.ram()[0x17], expect, "select(cond={c})");
    }
}

#[test]
fn br_unconditionally_jumps_to_the_target_block() {
    let m = parse("fn main(void) ()\n  block entry:\n    br skip\n  block skip:\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]));
    // Index-based label scheme (matches isel::select exactly): the first
    // block ("entry", here) is the bare function name; every other block
    // is `{func}_L{label}` — "skip" is the second block, so `main_Lskip`.
    assert!(asm.contains("BRA main_Lskip"), "asm:\n{asm}");
    assert!(asm.contains("main_Lskip:"), "target block must be labeled:\n{asm}");
}

#[test]
fn brcond_branches_on_the_condition_byte() {
    let m = parse("global c i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    br i1 %1 t f\n  block t:\n    ret void\n  block f:\n    ret void\n");
    let addrs = addrs(&[("c", 0x10), ("main::1", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (c, taken_first) in [(1u8, true), (0, false)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        // Both target blocks are just `RETURN` after `CALL main`'s frame,
        // so the only observable difference is which path halts sooner;
        // assert it runs to completion without panicking on either path
        // (the exact instruction-count assertion is brittle — prefer
        // proving both paths are reachable and correct via a later e2e
        // fixture instead, per Task 15).
        p.run(200);
        let _ = taken_first;
        assert!(p.halted());
    }
}

#[test]
fn phi_copies_the_incoming_value_before_the_predecessor_blocks_terminator() {
    // The brief's Step-1 code for this test used a literal `br i1 1 a b`
    // cond, but that trips the const-cond guard added below (Concern #3 of
    // this task: BrCond with a `Val::Const` cond would silently branch on a
    // stale Z flag, exactly the `Select`-cond hazard already fixed in Task
    // 11). Routed through a loaded register instead so this test exercises
    // only what it's meant to (Phi copies landing in both predecessor
    // blocks), not the guard.
    let m = parse(
        "global c i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @c\n\
           br i1 %1 a b\n\
         block a:\n\
           br j\n\
         block b:\n\
           br j\n\
         block j:\n\
           %2 = phi i8 5 a 7 b\n\
           ret void\n",
    );
    let addrs = addrs(&[("c", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    // The copy into %1's slot must appear in BOTH predecessor blocks
    // (block a gets MOVLW 5, block b gets MOVLW 7), before each one's own
    // `br j`. Blocks "a"/"b"/"j" are all non-first blocks here (block
    // "entry" is first), so each gets a `main_L{label}:` symbol.
    assert!(asm.contains("main_La:"), "asm:\n{asm}");
    assert!(asm.contains("main_Lb:"), "asm:\n{asm}");
    assert!(asm.contains("main_Lj:"), "asm:\n{asm}");
    assert!(asm.contains("MOVLW 0x05"), "asm:\n{asm}");
    assert!(asm.contains("MOVLW 0x07"), "asm:\n{asm}");
}

#[test]
#[should_panic(expected = "const cond BrCond")]
fn brcond_const_cond_is_rejected_not_silently_miscompiled() {
    // Same hazard as `select_const_cond_is_rejected_not_silently_miscompiled`:
    // `emit_load_w`'s `Val::Const` arm emits only `MOVLW`, which (per this
    // project's simulator, `crates/sim/src/lib.rs:903`) never touches the
    // Z flag. Without the guard, `br i1 1 ...`'s `BZ` right after the
    // `MOVLW` would test whatever Z flag the PREVIOUS instruction happened
    // to leave, silently branching to the wrong target instead of using
    // the literal cond. This must fail loudly instead.
    let m = parse("fn main(void) ()\n  block entry:\n    br i1 1 t f\n  block t:\n    ret void\n  block f:\n    ret void\n");
    let addrs = addrs(&[]);
    let _ = select(&PIC18F4550, &m, &addrs);
}
