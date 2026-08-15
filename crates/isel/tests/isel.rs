use isel::select;
use ir::parse;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u8)]) -> HashMap<String, u8> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn emits_add_for_in_plus_one() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("in".to_string(), 0x20u8);
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x20, W"));
    assert!(asm.contains("ADDLW 0x01"));
    assert!(asm.contains("MOVWF 0x21"));
}

#[test]
fn store_const_emits_movlw_not_movf() {
    let m = parse("global out i8\nfn main() -> void\n  block entry:\n    store i8 5 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVLW 0x05"), "expected MOVLW for const store:\n{asm}");
    assert!(asm.contains("MOVWF 0x21"), "expected MOVWF to @out:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
fn add_const_lhs_uses_addlw() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %x = add i8 5, %1\n    store i8 %x @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("in".to_string(), 0x20u8);
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("ADDLW 0x05"), "const-LHS add should use the ADDLW path:\n{asm}");
    assert!(!asm.contains("ADDWF 0x05"), "const must not be read as a file register:\n{asm}");
    assert!(!asm.contains("MOVF 0x05"), "const must not be read as a file register:\n{asm}");
}

#[test]
#[should_panic(expected = "only i8/i16 loads supported")]
fn panics_on_i1_load() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    %1 = load i1 @in\n    ret void\n");
    select(&m, &HashMap::new());
}

#[test]
fn add16_reg_reg_emits_carry_chain() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %r = add i16 %a, %b\n    store i16 %r @out\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x20), ("y", 0x22), ("out", 0x24)]);
    let asm = select(&m, &addrs);
    // %a=0x70/%b=0x72/%r=0x74: lo byte add then hi byte add with carry in.
    assert!(asm.contains("MOVF 0x72, W"), "add b_lo:\n{asm}");
    assert!(asm.contains("ADDWF 0x70, W"), "add a_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x74"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVF 0x73, W"), "add b_hi:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 0"), "carry test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "carry in add:\n{asm}");
    assert!(asm.contains("ADDWF 0x71, W"), "add a_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x75"), "store d_hi:\n{asm}");
}

#[test]
fn add16_reg_const_emits_carry_chain() {
    // 515 = 0x0203 -> lo 0x03, hi 0x02 (hi differs from the carry ADDLW 0x01,
    // so the k_hi add line is distinguishable).
    let m = parse(
        "global in i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @in\n    %r = add i16 %a, 515\n    store i16 %r @out\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22)]);
    let asm = select(&m, &addrs);
    // %a=0x70/%r=0x72.
    assert!(asm.contains("MOVF 0x70, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("ADDLW 0x03"), "add k_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x72"), "store d_lo:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 0"), "carry test:\n{asm}");
    assert!(asm.contains("ADDLW 0x01"), "carry in add:\n{asm}");
    assert!(asm.contains("ADDLW 0x02"), "add k_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x73"), "store d_hi:\n{asm}");
}

#[test]
fn i16_slot_avoids_straddling_common_boundary() {
    // Consume all of common RAM (0x70..=0x7F) with i8 values, then allocate
    // an i16: it must land entirely in bank-0 GPRs (>= 0x25), not straddle
    // 0x7F/0x80 (0x80 would alias bank-1 INDF).
    let globals: String = (0..16).map(|i| format!("global g{i} i8\n")).collect();
    let loads: String = (0..16).map(|i| format!("    %a{i} = load i8 @g{i}\n")).collect();
    let m = parse(&format!(
        "{globals}global out i16\nfn main() -> void\n  block entry:\n{loads}    %r = load i16 @out\n    store i16 %r @out\n    ret void\n"
    ));
    let mut addrs: HashMap<String, u8> = (0..16).map(|i| (format!("g{i}"), 0x20 + i)).collect();
    addrs.insert("out".to_string(), 0x30u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVWF 0x25"), "i16 lo should land at bank-0 0x25:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "i16 hi should land at bank-0 0x26:\n{asm}");
    assert!(!asm.contains("MOVWF 0x80"), "must not emit a write to 0x80 (bank-1 INDF):\n{asm}");
}

#[test]
fn and16_reg_const_uses_andlw() {
    // 4660 = 0x1234 -> lo 0x34, hi 0x12.
    let m = parse(
        "global in i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @in\n    %r = and i16 %a, 4660\n    store i16 %r @out\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x22)]);
    let asm = select(&m, &addrs);
    // %a=0x70/%r=0x72.
    assert!(asm.contains("MOVF 0x70, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("ANDLW 0x34"), "and k_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x72"), "store d_lo:\n{asm}");
    assert!(asm.contains("MOVF 0x71, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("ANDLW 0x12"), "and k_hi:\n{asm}");
    assert!(asm.contains("MOVWF 0x73"), "store d_hi:\n{asm}");
}

#[test]
fn zext_trunc_pair() {
    let m = parse(
        "global in i8\nglobal out16 i16\nglobal out8 i8\nfn main() -> void\n  block entry:\n    %v = load i8 @in\n    %z = zext i8 %v to i16\n    store i16 %z @out16\n    %t = trunc i16 %z to i8\n    store i8 %t @out8\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out16", 0x21), ("out8", 0x23)]);
    let asm = select(&m, &addrs);
    // %v=0x70, %z lo=0x71 hi=0x72, %t=0x73.
    assert!(asm.contains("MOVF 0x70, W"), "zext copies v:\n{asm}");
    assert!(asm.contains("MOVWF 0x71"), "zext stores d_lo:\n{asm}");
    assert!(asm.contains("CLRF 0x72"), "zext zeroes d_hi:\n{asm}");
    assert!(asm.contains("MOVF 0x71, W"), "trunc reads z_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x73"), "trunc stores d:\n{asm}");
}

#[test]
fn phi_copy_lands_before_terminator_of_each_predecessor() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    br merge\n  block thenb:\n    %b = load i16 @y\n    br merge\n  block merge:\n    %p = phi i16 %a entry %b thenb\n    store i16 %p @out\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x20), ("y", 0x22), ("out", 0x24)]);
    let asm = select(&m, &addrs);
    // %p reserved 0x70; %a=0x72 (hi 0x73), %b=0x74 (hi 0x75).
    // In block `entry` the copy of %a (ending MOVWF 0x71) precedes its GOTO.
    assert!(
        asm.contains("MOVWF 0x71\n    GOTO main_Lmerge"),
        "copy must land before the entry terminator:\n{asm}"
    );
    // In block `thenb` the copy of %b (ending MOVWF 0x71) precedes its GOTO.
    assert!(
        asm.contains("MOVF 0x74, W\n    MOVWF 0x70\n    MOVF 0x75, W\n    MOVWF 0x71\n    GOTO main_Lmerge"),
        "copy must land before the thenb terminator:\n{asm}"
    );
    // The merge block reads the phi destination (0x70 lo / 0x71 hi).
    assert!(asm.contains("MOVF 0x70, W"), "merge reads %p lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x24"), "merge stores %p lo to @out:\n{asm}");
}

#[test]
fn icmp_eq_i8_materializes_i1() {
    let m = parse(
        "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    store i8 %c @out\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let asm = select(&m, &addrs);
    // %1=0x70, %c=0x71, scratch=0x2A (after the driver's in/out globals 0x20/0x21).
    assert!(asm.contains("MOVF 0x70, W"), "load a:\n{asm}");
    assert!(asm.contains("XORLW 0x01"), "xor with const b:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store xor to scratch:\n{asm}");
    assert!(asm.contains("MOVLW 0x00"), "materialize 0:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 2"), "Z test:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x71"), "store d:\n{asm}");
}

#[test]
fn icmp_eq_i16_uses_scratch_accumulation() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp eq i16 %a, %b\n    store i8 %c @out\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x20), ("y", 0x22), ("out", 0x24)]);
    let asm = select(&m, &addrs);
    // %a=0x70/71, %b=0x72/73, %c=0x74, scratch=0x2A.
    assert!(asm.contains("MOVF 0x70, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("XORWF 0x72, W"), "xor b_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store lo xor to scratch:\n{asm}");
    assert!(asm.contains("MOVF 0x71, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("XORWF 0x73, W"), "xor b_hi:\n{asm}");
    assert!(asm.contains("IORWF 0x2A, W"), "or hi into scratch:\n{asm}");
    assert!(asm.contains("MOVWF 0x2A"), "store accumulated scratch:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x74"), "store d:\n{asm}");
}

#[test]
fn brcond_and_select_emit_skip_lines() {
    let m = parse(
        "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    %s = select i1 %c i8 10 i8 20\n    br i1 %c then end\n  block then:\n    store i8 %s @out\n    br end\n  block end:\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let asm = select(&m, &addrs);
    // %1=0x70, %c=0x71, %s=0x72, scratch=0x2A.
    // brcond: cond==0 -> main_Lend (f), cond!=0 -> main_Lthen (t).
    assert!(asm.contains("MOVF 0x71, W"), "brcond reads cond:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 2"), "brcond Z test:\n{asm}");
    assert!(asm.contains("GOTO main_Lend"), "brcond f:\n{asm}");
    assert!(asm.contains("GOTO main_Lthen"), "brcond t:\n{asm}");
    // select: test cond, jump to else, copy a=10 then b=20.
    assert!(asm.contains("GOTO tmp0"), "select else jump:\n{asm}");
    assert!(asm.contains("MOVLW 0x0A"), "select copy a:\n{asm}");
    assert!(asm.contains("MOVWF 0x72"), "select dst:\n{asm}");
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
         fn main() -> void\n  block entry:\n\
           %1 = load i8 @a\n    %c1 = icmp eq i8 %1, 0\n\
           %s1 = select i1 %c1 i8 1 i8 2\n    store i8 %s1 @o1\n    ret void\n\
         fn f2() -> void\n  block entry:\n\
           %2 = load i8 @b\n    %c2 = icmp eq i8 %2, 0\n\
           %s2 = select i1 %c2 i8 3 i8 4\n    store i8 %s2 @o2\n    ret void\n",
    );
    let addrs = addrs(&[("a", 0x20), ("b", 0x21), ("o1", 0x22), ("o2", 0x23)]);
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
fn slot_skips_icmp_scratch_0x2a() {
    // 16 i8 loads fill common RAM (0x70..=0x7F); bank-0 GPRs then continue
    // from 0x25. The 22nd byte-slot would land on 0x2A, the fixed icmp
    // scratch byte. slot() must skip it so an icmp in the same function never
    // silently corrupts that slot.
    let loads: String = (0..21).map(|i| format!("    %a{i} = load i8 @in\n")).collect();
    let m = parse(&format!(
        "global in i8\nfn main() -> void\n  block entry:\n{loads}    %a21 = add i8 %a20, 1\n    ret void\n"
    ));
    let addrs = addrs(&[("in", 0x20)]);
    let asm = select(&m, &addrs);
    // a0..a15 -> 0x70..0x7F, a16..a20 -> 0x25..0x29; the 22nd slot (a21) must
    // skip the scratch 0x2A and land at 0x2B.
    assert!(asm.contains("MOVWF 0x2B"), "22nd slot should land at 0x2B, not the scratch:\n{asm}");
    assert!(!asm.contains("MOVWF 0x2A"), "no slot may write the icmp scratch 0x2A:\n{asm}");
}
