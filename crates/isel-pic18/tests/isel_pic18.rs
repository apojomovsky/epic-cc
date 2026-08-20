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
fn zext_i1_to_i8_same_width_widen_compiles_and_runs() {
    // Regression test: `zext i1 to i8` (e.g. `u8 b = (a < b);`) is legal
    // and common — i1 and i8 both report `.bytes() == 1` in the byte
    // model, so this is a same-width "widen" that's really a 1-byte copy.
    // A prior version of this guard required `to.bytes() > from.bytes()`
    // (strictly wider), which panicked on this case even though it's not
    // a narrowing bug. The guard must accept `to.bytes() >= from.bytes()`.
    let m = parse(
        "global a i8\nglobal b i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    %4 = zext i1 %3 to i8\n    store i8 %4 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x10), ("b", 0x11), ("out", 0x12),
        ("main::1", 0x13), ("main::2", 0x14), ("main::3", 0x15), ("main::4", 0x16),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [(5u8, 5u8, 1u8), (5, 6, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.run(200);
        assert_eq!(p.ram()[0x12], expect, "zext(icmp eq({av},{bv}))");
    }
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
    // Each target block stores a DISTINGUISHABLE value to @out (matching
    // the pattern `select_picks_a_when_cond_is_true_and_b_otherwise` uses
    // for `Select`) so this test actually proves which way `BZ`/`BRA`
    // branch, not just that some path halts without panicking — a
    // polarity inversion (`BZ` going to the wrong target) would still
    // pass a "both paths halt" check but fails this one.
    let m = parse("global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    br i1 %1 t f\n  block t:\n    store i8 1 @out\n    ret void\n  block f:\n    store i8 2 @out\n    ret void\n");
    let addrs = addrs(&[("c", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (c, expect) in [(1u8, 1u8), (0, 2)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        p.run(200);
        assert!(p.halted());
        assert_eq!(p.ram()[0x11], expect, "brcond(cond={c}) took the wrong target");
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
    // Bare `asm.contains(...)` above doesn't confirm WHICH block each copy
    // landed in — split the asm on block labels and check per-section, so
    // a copy landing in the wrong predecessor's section (or in neither)
    // would fail this test even though the whole-file `contains` checks
    // above would still pass.
    let a_section = block_section(&asm, "main_La");
    let b_section = block_section(&asm, "main_Lb");
    assert!(a_section.contains("MOVLW 0x05"), "block a's section:\n{a_section}");
    assert!(!a_section.contains("MOVLW 0x07"), "block a's section must not contain b's phi copy:\n{a_section}");
    assert!(b_section.contains("MOVLW 0x07"), "block b's section:\n{b_section}");
    assert!(!b_section.contains("MOVLW 0x05"), "block b's section must not contain a's phi copy:\n{b_section}");
}

/// The lines belonging to one block's label, up to (but excluding) the
/// next line that looks like a label (ends with `:`). Used to check that
/// per-predecessor phi copies land in the RIGHT block's section, not just
/// somewhere in the whole function's asm.
fn block_section(asm: &str, label: &str) -> String {
    let marker = format!("{label}:");
    let lines: Vec<&str> = asm.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == marker)
        .unwrap_or_else(|| panic!("label {label} not found in:\n{asm}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.trim_end().ends_with(':'))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ret_with_a_value_writes_it_into_the_fixed_retval_region() {
    // `Ret(Some((ty, v)))` is new in this task — before this task, only
    // `Ret(None)` was handled at all (inside `emit_inst`, since it hadn't
    // moved into the terminator pass yet). It writes each byte of the
    // returned value into the fixed retval region (`device.common_ram`,
    // which is 0x0000 for `PIC18F4550`) before `RETURN`. Check both an i8
    // and an i16 return so both the single-byte and multi-byte loop paths
    // are exercised.
    let m = parse("fn main(void) ()\n  block entry:\n    ret i8 42\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]));
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(200);
    assert!(p.halted());
    assert_eq!(p.ram()[0x00], 42, "i8 retval should land at the fixed retval region (0x0000)");

    let m16 = parse("fn main(void) ()\n  block entry:\n    ret i16 4660\n"); // 4660 == 0x1234
    let asm16 = select(&PIC18F4550, &m16, &addrs(&[]));
    let words16 = asm::assemble_pic18(&asm16);
    let mut p16 = pic14_sim::Pic18::new(words16);
    p16.run(200);
    assert!(p16.halted());
    assert_eq!(p16.ram()[0x00], 0x34, "i16 retval low byte");
    assert_eq!(p16.ram()[0x01], 0x12, "i16 retval high byte");
}

#[test]
fn rotated_loop_exit_phi_reads_the_pre_increment_value_not_the_clobbered_one() {
    // Regression for the Critical bug found in task review of the first
    // Task 12 implementation: phi copies were keyed by PREDECESSOR alone,
    // so a `BrCond` whose two successors both consume phis ran BOTH
    // successors' copies unconditionally before the branch. On this
    // rotated-loop shape (the standard clang -O1 loop shape, and the one
    // `scalar.c` — a Task 15 e2e fixture — actually produces), that
    // clobbers the loop header's phi slot (`%2`) with the next-iteration
    // value BEFORE the exit block's phi (`%5`) gets a chance to read the
    // CURRENT one, so the exit block would read a wrong, clobbered value.
    //
    // Loop shape: `%2` (the header phi) starts at 0 (from `entry`), and
    // each iteration through `body` computes `%3 = %2 + 1`, loops back
    // while `%3 < 3` (feeding `%2 <- %3` on that back edge), and exits
    // once `%3 == 3` (feeding `%5 <- %2`, the CURRENT value BEFORE this
    // iteration's increment, into the exit block).
    //
    // Trace: iter1 %2=0,%3=1,cont(1<3); iter2 %2=1,%3=2,cont(2<3);
    // iter3 %2=2,%3=3,exit(3<3 false) -> %5 must be 2 (the value %2 held
    // going into the FINAL iteration), not 3 (the clobbered post-increment
    // value the original bug would have produced).
    let m = parse(
        "global out i8\nfn main(void) ()\n\
         block entry:\n\
           br body\n\
         block body:\n\
           %2 = phi i8 0 entry %3 body\n\
           %3 = add i8 %2, 1\n\
           %4 = icmp ult i8 %3, 3\n\
           br i1 %4 body exit\n\
         block exit:\n\
           %5 = phi i8 %2 body\n\
           store i8 %5 @out\n\
           ret void\n",
    );
    let addrs = addrs(&[("out", 0x10), ("main::2", 0x11), ("main::3", 0x12), ("main::4", 0x13), ("main::5", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x10], 2, "exit's phi must read %2's pre-increment value (2), not the clobbered next-iteration value (3)");
}

#[test]
fn brcond_both_edges_phi_copies_get_correct_bsr_after_the_synthesized_fcopies_label() {
    // Regression for a bug this fix round's own `(Some(ct), Some(cf))`
    // BrCond phi-copy handling introduced: the synthesized `l_fcopies`
    // label (reached via `BZ`, not a real block label) didn't reset
    // `Gen::bsr`, so if the t-edge's copy left the TRACKED `bsr` pointing
    // at bank 2, the f-edge's copy — which never actually runs the
    // t-edge's `MOVLB` at runtime when `BZ` is taken — would wrongly
    // believe BSR was already 2 and skip emitting its own `MOVLB`,
    // writing to the wrong physical bank.
    //
    // `main::1` (the cond, read from `%1`'s slot) sits in bank 1; both
    // phi destinations (`main::2` for the t edge, `main::3` for the f
    // edge) sit in bank 2 — the shape needed to trigger the hazard: the
    // cond load leaves REAL BSR=1, the t-edge's copy sets REAL BSR=2 only
    // when the t-edge actually executes, and the f-edge's copy must
    // independently re-establish BSR=2 rather than trusting a stale
    // tracked value left over from code-generation order.
    let m = parse(
        "global cond i8\nglobal outT i8\nglobal outF i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @cond\n\
           br i1 %1 t f\n\
         block t:\n\
           %2 = phi i8 7 entry\n\
           store i8 %2 @outT\n\
           ret void\n\
         block f:\n\
           %3 = phi i8 9 entry\n\
           store i8 %3 @outF\n\
           ret void\n",
    );
    let addrs = addrs(&[
        ("cond", 0x10),
        ("outT", 0x11),
        ("outF", 0x12),
        ("main::1", 0x101),
        ("main::2", 0x210),
        ("main::3", 0x211),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for (c, expect_t, expect_f) in [(1u8, 7u8, 0u8), (0, 0, 9)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        p.run(200);
        assert!(p.halted());
        assert_eq!(p.ram()[0x11], expect_t, "outT (bank-2 phi dest) after cond={c}");
        assert_eq!(p.ram()[0x12], expect_f, "outF (bank-2 phi dest) after cond={c} — without the BSR reset at l_fcopies, this lands in the wrong bank on the cond=0 path");
    }
}

#[test]
fn select_l_end_resets_bsr_so_a_later_same_block_instruction_is_not_misbanked() {
    // Regression for a third instance of the same BSR-tracking hazard,
    // this time at `Select`'s MERGE label (`l_end`), found during the
    // systemic audit that added `Gen::emit_label`. `l_end` is reached two
    // ways: `BRA l_end` from the `a`-arm, and plain fallthrough from the
    // `b`-arm — and the two arms don't necessarily touch `bsr` the same
    // way. Here `s.a` is a REGISTER value (copied via `MOVFF`, which never
    // calls `operand()`/touches `bsr` at all), while `s.b` is a CONSTANT
    // (copied via `MOVLW`+`MOVWF`, which does call `operand()` for the
    // (shared) destination's bank). So after generating the whole
    // `Select`, the tracked `bsr` reflects the `b`-arm's bank
    // unconditionally — correct if the `b`-arm is the one that actually
    // ran, wrong if the `a`-arm ran instead (real BSR there is whatever
    // it was before the `Select`, since the `a`-arm's `MOVFF` never
    // touched it).
    //
    // `@out` is placed in the SAME bank as the `Select`'s destination
    // (bank 2) so the stale tracked value from the `b`-arm coincidentally
    // "matches" what `@out`'s store needs — exactly the condition needed
    // to make `operand()` wrongly skip the `MOVLB` on the `a`-path.
    let m = parse(
        "global cond i8\nglobal r i8\nglobal out i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @cond\n\
           %2 = load i8 @r\n\
           %3 = select i1 %1 i8 %2 i8 9\n\
           store i8 5 @out\n\
           ret void\n",
    );
    let addrs = addrs(&[
        ("cond", 0x10),
        ("r", 0x11),
        ("out", 0x211),  // bank 2, same bank as main::3 below
        ("main::1", 0x101), // bank 1: the cond load's own MOVLB
        ("main::2", 0x12),
        ("main::3", 0x210), // bank 2: the Select's dst (b-arm's MOVLB target)
    ]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    for c in [1u8, 0] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        p.ram_mut()[0x11] = 0x55;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x211], 5,
            "store after the Select (cond={c}) must land in @out's real bank-2 address, not get silently misbanked by a stale tracked BSR left over from Select's b-arm"
        );
    }
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

#[test]
fn call_copies_scalar_args_and_reads_the_retval_back() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %1 = call i8 @add1(i8 5)\n\
             ret void\n\
         fn add1(i8) (x=i8)\n\
           block entry:\n\
             %2 = add i8 %x, 1\n\
             ret i8 %2\n",
    );
    let addrs = addrs(&[("main::1", 0x10), ("add1::x", 0x11), ("add1::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(300);
    assert_eq!(p.ram()[0x10], 6, "main::1 gets add1(5)'s retval");
}

#[test]
fn call_return_invalidates_tracked_bsr_so_a_later_banked_access_is_not_misbanked() {
    // Regression for the final-review finding: `Gen.bsr` tracks the bank
    // the MOST RECENT *emitted* `MOVLB` set, but a `CALL` transfers control
    // to a callee that runs its own arbitrary `MOVLB`s and never restores
    // the caller's bank on `RETURN` — so the tracked value is stale the
    // instant control returns to the caller, and (unless invalidated)
    // `operand()` can wrongly elide a `MOVLB` the next banked access
    // actually needs.
    //
    // Shape, closely modeled on the reviewer's repro:
    //   main: MOVLB 0x1 (tracked bsr = Some(1)), ADDWF/MOVWF against bank-1
    //         locals (`main::1`/`main::2`/`main::3`, all >= 0x100) ->
    //         CALL f
    //   f:    its own ADDWF/MOVWF against BANK-2 locals (`f::1`/`f::2`/
    //         `f::3`, all >= 0x200) -> emits its own MOVLB 0x2, and never
    //         restores bank 1 before RETURN, so the REAL hardware BSR is 2
    //         when control returns to main.
    //   main: a second `add` reusing %1/%2 (bank-1 addresses again),
    //         storing into `main::4` (also bank 1).
    //
    // Without the fix, main's tracked `bsr` is still `Some(1)` after the
    // `CALL` (never invalidated) — and the post-call add's operands are
    // ALSO bank 1, so `operand()` sees tracked==target and wrongly skips
    // the `MOVLB`. But the REAL BSR at that point is 2 (left over from
    // `f`), so every banked access in the post-call add actually lands in
    // bank 2 (`f`'s slots) instead of bank 1 (main's own `%1`/`%2`/
    // `main::4`) — silently reading `f`'s operands and writing `main::4`'s
    // result into `f::3`'s address (0x213) instead of `main::4`'s real
    // address (0x113), leaving `main::4` untouched (0x00).
    //
    // With the fix, `self.bsr = None` after `CALL` forces a fresh `MOVLB
    // 0x1` before the post-call add touches anything, so it correctly
    // reads main's own bank-1 `%1`/`%2` and writes bank-1 `main::4`.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @f()\n\
             %4 = add i8 %1, %2\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             %2 = load i8 @d\n\
             %3 = add i8 %1, %2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("main::1", 0x110), // bank 1
        ("main::2", 0x111), // bank 1
        ("main::3", 0x112), // bank 1: pre-call add's dst, forces MOVLB 0x1
        ("main::4", 0x113), // bank 1: post-call add's dst — the one under test
        ("f::1", 0x210),    // bank 2
        ("f::2", 0x211),    // bank 2
        ("f::3", 0x212),    // bank 2: f's own add dst, forces MOVLB 0x2 for real
    ]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x20] = 3; // a
    p.ram_mut()[0x21] = 4; // b
    p.ram_mut()[0x22] = 0x11; // c (f's own operands; must not leak into main::4)
    p.ram_mut()[0x23] = 0x22; // d
    p.run(500);
    assert!(p.halted(), "asm:\n{asm}");
    assert_eq!(
        p.ram()[0x112], 7,
        "sanity: the pre-call add (main::3 = a+b) must still be correct:\nasm:\n{asm}"
    );
    assert_eq!(
        p.ram()[0x113], 7,
        "post-call add (main::4 = a+b, bank 1) must land at its real bank-1 \
         address using main's OWN operands, not get silently misbanked into \
         bank 2 (f's slots) by a stale tracked BSR left over from the CALL:\nasm:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "const byval call arg")]
fn call_const_byval_arg_is_rejected_not_silently_miscompiled() {
    // A `byval` arg means "copy N bytes from the address `arg.val` points
    // to" — the IR's text parser has no type-level restriction preventing
    // a bare literal there (`parse_call_arg` accepts any `parse_val`
    // result after the `byvalN` keyword), and `val_addr`'s `Val::Const`
    // arm would silently reinterpret the literal as a RAM address
    // (`k & 0xFF`) rather than reject it. Same hazard class as `Bin`'s
    // const-LHS guard; must fail loudly instead.
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @f(byval2 5)\n    ret void\n\
         fn f(void) (p=byval2)\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[("f::p", 0x10)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
fn a_byval_arg_through_a_geped_pointer_copies_from_the_right_offset() {
    // Passing &g.field (a struct-field GEP) as a byval arg must copy from
    // g's base + the field's offset, not from g's base.
    let m = parse(
        "global g i8\n\
         fn f(void) (p=byval2)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %fp = gep @g +2\n\
             call void @f(byval2 %fp)\n\
             ret void\n",
    );
    let addrs = addrs(&[("g", 0x100), ("f::p", 0x120)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(
        asm.contains("MOVFF 0x102, 0x120") || asm.contains("MOVFF 0x102,0x120"),
        "the byval copy must start at g+2 (0x102), not g's base (0x100):\n{asm}"
    );
}

#[test]
#[should_panic(expected = "const sret call arg")]
fn call_const_sret_arg_is_rejected_not_silently_miscompiled() {
    // Same hazard as the byval case above: an `sret` arg is always meant
    // to be a 2-byte pointer, but the IR's text parser doesn't prevent a
    // bare literal after the `sret` keyword, and `val_addr`'s
    // `Val::Const` arm would silently treat the literal as an address
    // instead of rejecting it.
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @f(sret 5)\n    ret void\n\
         fn f(void) (p=sret)\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[("f::p", 0x10)]);
    let _ = select(&PIC18F4550, &m, &addrs);
}

#[test]
fn a_gep_with_a_constant_offset_and_no_dynamic_term_loads_directly() {
    // arr[2] with a CONST index folds to a plain direct address, no FSR,
    // no LFSR, just a MOVFF at the folded address (base + 2).
    let m = parse(
        "global arr i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %p = gep @arr +2\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[("arr", 0x100), ("out", 0x110), ("main::v", 0x111)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(
        asm.contains("MOVFF 0x102, 0x111") || asm.contains("MOVFF 0x102,0x111"),
        "arr[2] must read directly from base+2 (0x102), no FSR machinery:\n{asm}"
    );
    assert!(!asm.contains("LFSR"), "a fully-constant GEP needs no FSR setup:\n{asm}");
}

#[test]
fn a_dynamic_index_sets_fsr0_and_reads_through_indf0() {
    // ram[i]: base = @ram (0x120), k = 0, terms = [(1, "i")] (scale 1,
    // a byte array). Must LFSR the base then read through INDF0, no
    // constant-offset direct MOVFF this time.
    let m = parse(
        "global ram i8\n\
         global out i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @ram +0 +1*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[("ram", 0x120), ("out", 0x130), ("idx", 0x131), ("main::i", 0x132), ("main::v", 0x133)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(asm.contains("LFSR 0, 0x120") || asm.contains("LFSR 0,0x120"), "must seed FSR0 with the array base:\n{asm}");
    assert!(asm.contains("0xFEF") || asm.contains("INDF0"), "must read through INDF0:\n{asm}");
}

#[test]
fn a_scale_2_dynamic_index_unrolls_two_adds() {
    // A u16 array element: ram16[i] with element width 2: the offset
    // into the array is 2*i, unrolled as two ADDWFs onto FSR0L (with
    // carry into FSR0H), mirroring PIC14's emit_accum_terms.
    let m = parse(
        "global ram16 i16\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @ram16 +0 +2*%i\n\
             %v = load i16 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[("ram16", 0x140), ("idx", 0x150), ("main::i", 0x151), ("main::v", 0x152)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    let addwf_to_fsr0l = asm.matches("ADDWF 0x0E9").count() + asm.matches("ADDWF 0x0e9").count();
    assert!(addwf_to_fsr0l >= 2, "a scale-2 term must unroll two adds onto FSR0L:\n{asm}");
}

#[test]
fn an_sret_return_writes_through_the_callers_address() {
    // struct Pair mk(...) { r.a = a; return r; }: inside mk, `r` is an
    // sret param: its SLOT holds the caller's target address, and every
    // field store through it must go via FSR0/INDF0, never a direct
    // write to the slot's own address (that would corrupt the pointer).
    let m = parse(
        "fn mk(void) (r=sret)\n\
           block entry:\n\
             store i8 5 %r\n\
             ret void\n",
    );
    let addrs = addrs(&[("mk::r", 0x160)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    assert!(
        !asm.contains("LFSR 0, 0x160") && !asm.contains("LFSR 0,0x160"),
        "the store target is INSIDE the pointer at 0x160, not the literal address 0x160:\n{asm}"
    );
    assert!(asm.contains("0xFEF") || asm.contains("INDF0"), "an sret store must go through INDF0:\n{asm}");
}

#[test]
fn a_const_length_memcpy_copies_byte_by_byte() {
    // Whole-struct assignment (`g = mk(...)` in structs.c) lowers to a
    // constant-length memcpy: three MOVFF src+i -> dst+i byte copies, no
    // loop and no FSR for direct global addresses (mirrors PIC14's
    // `memcpy_emits_byte_pairs`, crates/isel/tests/isel.rs).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 3\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    for i in 0..3u16 {
        let expect = format!("MOVFF 0x{:03X}, 0x{:03X}", 0x100 + i, 0x110 + i);
        let expect_nospace = format!("MOVFF 0x{:03X},0x{:03X}", 0x100 + i, 0x110 + i);
        assert!(asm.contains(&expect) || asm.contains(&expect_nospace), "byte {i} missing:\n{asm}");
    }
}

#[test]
#[should_panic(expected = "not yet supported")]
fn a_dynamic_length_memcpy_panics_loudly() {
    // A runtime length would need a loop; P3's scope is constant-length
    // memcpy only, so a `MemLen::Reg` must panic loudly rather than
    // silently copy a wrong number of bytes.
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global n i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %len = load i8 @n\n\
             memcpy @dst @src %len\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110), ("n", 0x120), ("main::len", 0x121)]);
    select(&PIC18F4550, &m, &addrs);
}

#[test]
fn alloca_and_gep_emit_nothing_of_their_own() {
    let m = parse(
        "global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %buf = alloca 4\n\
             %p = gep %buf +1\n\
             store i8 9 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[("out", 0x100), ("main::buf", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs);
    // The store must land at buf+1 (0x111): proof both Alloca and Gep
    // were handled (the seed + the fold), not merely "didn't crash."
    // 0x111 is bank 1, f=0x11, so the banked emission is MOVLB 0x1 +
    // MOVWF 0x011,B (the literal "0x111" never appears in the asm).
    assert!(
        asm.contains("MOVLB 0x1") && asm.contains("MOVWF 0x011,B"),
        "store must target buf+1 (0x111):\n{asm}"
    );
}
