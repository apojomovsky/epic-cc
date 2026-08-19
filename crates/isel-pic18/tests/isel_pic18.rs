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
fn icmp_ult_and_uge_use_the_carry_flag() {
    for (pred, a, b, expect) in [("ult", 3u8, 5u8, 1u8), ("ult", 5, 3, 0), ("uge", 5, 3, 1), ("uge", 3, 5, 0)] {
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
        ("sge", 1, 0xFF, 1),
        ("sge", 0xFF, 1, 0),
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
