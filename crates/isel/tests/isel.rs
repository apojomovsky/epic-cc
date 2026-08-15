use isel::select;
use ir::parse;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u8)]) -> HashMap<String, u8> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn emits_add_for_in_plus_one() {
    // Milestone 3: locals come from the map too, keyed `{func}::{name}`.
    // alloc: globals in=0x20/out=0x21 -> end_of_globals 0x22 -> the root
    // frame starts at 0x25, so main's locals land at 0x25/0x26.
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
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
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    %1 = load i1 @in\n    ret void\n");
    select(&m, &HashMap::new());
}

#[test]
#[should_panic(expected = "no slot for main::1")]
fn panics_when_local_address_missing_from_map() {
    // Every local address comes from the map; a missing entry must fail
    // loudly instead of allocating a slot internally.
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let _ = select(&m, &addrs);
}

#[test]
fn add16_reg_reg_emits_carry_chain() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %r = add i16 %a, %b\n    store i16 %r @out\n    ret void\n",
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
        "global in i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @in\n    %r = add i16 %a, 515\n    store i16 %r @out\n    ret void\n",
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
        "{globals}global out i16\nfn main() -> void\n  block entry:\n{loads}    %r = load i16 @out\n    store i16 %r @out\n    ret void\n"
    ));
    let mut addrs: HashMap<String, u8> = (0..16).map(|i| (format!("g{i}"), 0x20 + i)).collect();
    addrs.insert("out".to_string(), 0x30u8);
    for i in 0..16 {
        addrs.insert(format!("main::a{i}"), 0x35 + i);
    }
    addrs.insert("main::r".to_string(), 0x45u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVWF 0x45"), "i16 lo should land at map address 0x45:\n{asm}");
    assert!(asm.contains("MOVWF 0x46"), "i16 hi should land at map address 0x46:\n{asm}");
    assert!(asm.contains("MOVF 0x46, W"), "store reads the i16 hi from 0x46:\n{asm}");
    assert!(!asm.contains("MOVWF 0x80"), "must not emit a write to 0x80 (bank-1 INDF):\n{asm}");
}

#[test]
fn and16_reg_const_uses_andlw() {
    // 4660 = 0x1234 -> lo 0x34, hi 0x12.
    let m = parse(
        "global in i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @in\n    %r = and i16 %a, 4660\n    store i16 %r @out\n    ret void\n",
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
        "global in i8\nglobal out16 i16\nglobal out8 i8\nfn main() -> void\n  block entry:\n    %v = load i8 @in\n    %z = zext i8 %v to i16\n    store i16 %z @out16\n    %t = trunc i16 %z to i8\n    store i8 %t @out8\n    ret void\n",
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
fn phi_copy_lands_before_terminator_of_each_predecessor() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i16\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    br merge\n  block thenb:\n    %b = load i16 @y\n    br merge\n  block merge:\n    %p = phi i16 %a entry %b thenb\n    store i16 %p @out\n    ret void\n",
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
        "global x i8\nglobal y i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %a = load i8 @x\n    br merge\n  block merge:\n    %p = phi i8 %a entry\n    %q = phi i8 %p entry\n    store i8 %q @out\n    ret void\n",
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
        "global x i8\nglobal y i8\nfn main() -> void\n  block entry:\n    %a = load i8 @x\n    br merge\n  block merge:\n    %p = phi i8 %q entry\n    %q = phi i8 %p entry\n    ret void\n",
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
        "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    store i8 %c @out\n    ret void\n",
    );
    // alloc: root frame at 0x25; %1=0x25, %c=0x26.
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::c", 0x26),
    ]);
    let asm = select(&m, &addrs);
    // %1=0x25, %c=0x26, scratch=0x22 (end_of_globals: 0x20+1, 0x21+1 -> 0x22).
    assert!(asm.contains("MOVF 0x25, W"), "load a:\n{asm}");
    assert!(asm.contains("XORLW 0x01"), "xor with const b:\n{asm}");
    assert!(asm.contains("MOVWF 0x22"), "store xor to scratch:\n{asm}");
    assert!(asm.contains("MOVLW 0x00"), "materialize 0:\n{asm}");
    assert!(asm.contains("BTFSC STATUS, 2"), "Z test:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x26"), "store d:\n{asm}");
}

#[test]
fn icmp_eq_i16_uses_scratch_accumulation() {
    let m = parse(
        "global x i16\nglobal y i16\nglobal out i8\nfn main() -> void\n  block entry:\n    %a = load i16 @x\n    %b = load i16 @y\n    %c = icmp eq i16 %a, %b\n    store i8 %c @out\n    ret void\n",
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
    // %a=0x28/29, %b=0x2A/2B, %c=0x2C, scratch=0x25 (end_of_globals:
    // max(0x20+2, 0x22+2, 0x24+1) = 0x25).
    assert!(asm.contains("MOVF 0x28, W"), "load a_lo:\n{asm}");
    assert!(asm.contains("XORWF 0x2A, W"), "xor b_lo:\n{asm}");
    assert!(asm.contains("MOVWF 0x25"), "store lo xor to scratch:\n{asm}");
    assert!(asm.contains("MOVF 0x29, W"), "load a_hi:\n{asm}");
    assert!(asm.contains("XORWF 0x2B, W"), "xor b_hi:\n{asm}");
    assert!(asm.contains("IORWF 0x25, W"), "or hi into scratch:\n{asm}");
    assert!(asm.contains("MOVWF 0x25"), "store accumulated scratch:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "materialize 1:\n{asm}");
    assert!(asm.contains("MOVWF 0x2C"), "store d:\n{asm}");
}

#[test]
fn brcond_and_select_emit_skip_lines() {
    let m = parse(
        "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %c = icmp eq i8 %1, 1\n    %s = select i1 %c i8 10 i8 20\n    br i1 %c then end\n  block then:\n    store i8 %s @out\n    br end\n  block end:\n    ret void\n",
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
         fn main() -> void\n  block entry:\n\
           %1 = load i8 @a\n    %c1 = icmp eq i8 %1, 0\n\
           %s1 = select i1 %c1 i8 1 i8 2\n    store i8 %s1 @o1\n    ret void\n\
         fn f2() -> void\n  block entry:\n\
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
    // The icmp scratch byte sits at end_of_globals, with the two retval
    // bytes just after it. isel does not allocate slots any more, so a local
    // is used at exactly the map address alloc provides — here 0x73/0x74,
    // past the scratch and retval bytes (alloc's frame starts at
    // bank0_start = end_of_globals + 3).
    let m = parse(
        "global in i8\nfn main() -> void\n  block entry:\n\
           %a0 = load i8 @in\n    %c = icmp eq i8 %a0, 0\n    ret void\n",
    );
    let addrs = addrs(&[("in", 0x6F), ("main::a0", 0x73), ("main::c", 0x74)]);
    let asm = select(&m, &addrs);
    // end_of_globals = 0x6F + 1 = 0x70: scratch 0x70, retval 0x71/0x72.
    assert!(asm.contains("MOVWF 0x70"), "icmp writes the scratch at end_of_globals:\n{asm}");
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
         fn add(i16 %x, i16 %y) -> i16\n  block entry:\n\
           %r = add i16 %x, %y\n    ret i16 %r\n\
         fn main() -> void\n  block entry:\n\
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
    // Retval copy: retval_lo/hi (0x27/0x28) -> %3 (0x2D/0x2E).
    assert!(
        asm.contains("MOVF 0x27, W\n    MOVWF 0x2D\n    MOVF 0x28, W\n    MOVWF 0x2E"),
        "copy retval into %3:\n{asm}"
    );
}

#[test]
fn ret_i16_copies_value_to_retval_and_returns() {
    // ret i16 %v: copy %v into the retval slots (end_of_globals+1/+2) then
    // RETURN.
    let m = parse(
        "global x i16\nfn main() -> i16\n  block entry:\n\
           %v = load i16 @x\n    ret i16 %v\n",
    );
    // alloc: root frame at 0x25; %v=0x25.
    let addrs = addrs(&[("x", 0x20), ("main::v", 0x25)]);
    let asm = select(&m, &addrs);
    // end_of_globals = 0x20+2 = 0x22: retval 0x23/0x24. %v = 0x25 (hi 0x26).
    assert!(
        asm.contains("MOVF 0x25, W\n    MOVWF 0x23\n    MOVF 0x26, W\n    MOVWF 0x24"),
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
         fn add(i16 %x, i16 %y) -> i16\n  block entry:\n\
           %r = add i16 %x, %y\n    ret i16 %r\n\
         fn main() -> void\n  block entry:\n\
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
