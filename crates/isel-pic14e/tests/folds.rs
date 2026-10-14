//! Single-use load folds on PIC14E (epic-cc#902): loads whose byte never
//! stages to an overlay slot. Hand-written IR keeps each test
//! deterministic; `alloc` drops the same slots these tests omit, so a
//! backend lookup of a dropped slot would panic here instead of passing.

use device::PIC16F1937;
use ir::parse;
use isel_pic14e::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn sim_run_bytes(
    ir_text: &str,
    map: &[(&str, u16)],
    seed: &[(u16, u8)],
    out: u16,
    n: usize,
) -> Vec<u8> {
    let m = parse(ir_text);
    let asm = select(&PIC16F1937, &m, &addrs(map));
    let words = asm::assemble_pic14e(&asm);
    let mut p = pic14_sim::Pic14e::with_device(&PIC16F1937, words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    (0..n).map(|i| p.ram()[out as usize + i]).collect()
}

#[test]
fn add32_const_folds_carry_without_park() {
    // epic-cc#1078: const bytes other than 0xFF fold the carry into the
    // constant in W (ADDLW 1), so no scratch park is needed.
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = add i32 %a, 67305985\n    store i32 %r @out\n    ret void\n",
    );
    // 67305985 = 0x04030201: bytes 1..3 all fold.
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[("x", 0x20), ("out", 0x24), ("main::r", 0x38)]),
    );
    assert!(
        asm.contains("    MOVF 0x21, W\n    MOVWF 0x39\n    MOVLW 0x02\n    BTFSC STATUS, 0 ; C\n    ADDLW 0x01\n    ADDWF 0x39, F"),
        "byte 1 const carry folds without a park:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x23, W\n    MOVWF 0x3B\n    MOVLW 0x04\n    BTFSC STATUS, 0 ; C\n    ADDLW 0x01\n    ADDWF 0x3B, F"),
        "byte 3 const carry folds without a park:\n{asm}"
    );
}

#[test]
fn add32_const_ff_byte_folds_without_park() {
    // epic-cc#1192: a 0xFF const byte plus carry is a_i - 1 + carry, so
    // subtracting the 1 only when carry is clear keeps the true
    // carry-out (a skipped SUBWF keeps C = 1) with no scratch park.
    let m = parse(
        "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = add i32 %a, 16711681\n    store i32 %r @out\n    ret void\n",
    );
    // 16711681 = 0x00FF0001: byte 2 is the 0xFF fold.
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[("x", 0x20), ("out", 0x24), ("main::r", 0x38)]),
    );
    assert!(
        asm.contains("    MOVF 0x22, W\n    MOVWF 0x3A\n    MOVLW 0x01\n    BTFSS STATUS, 0 ; C\n    SUBWF 0x3A, F"),
        "0xFF byte folds without a park:\n{asm}"
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no const park remains in this add:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0x21, W\n    MOVWF 0x39\n    MOVLW 0x00\n    BTFSC STATUS, 0 ; C\n    ADDLW 0x01\n    ADDWF 0x39, F"),
        "0x00 byte folds without a park:\n{asm}"
    );
}
fn add32_reg_reg_module() -> String {
    "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = add i32 %a, %b\n    store i32 %r @out\n    ret void".to_string()
}

#[test]
fn add32_reg_reg_carry_folds_without_park() {
    // epic-cc#1255: the carry folds onto b's own file byte via INCFSZ's
    // skip, so no scratch park is needed when dst leaves b addressable
    // in the same bank. All bytes here are bank-0 with no overlay.
    let m = parse(&add32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x34),
        ]),
    );
    assert!(
        asm.contains("    MOVF 0x2D, W\n    MOVWF 0x35\n    MOVF 0x25, W\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x25, W\n    ADDWF 0x35, F"),
        "byte 1 park-free carry fold:\n{asm}"
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no scratch carry park remains:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF 0x70"),
        "no scratch park store remains:\n{asm}"
    );
}

#[test]
fn add32_reg_reg_overlay_and_cross_bank_keep_park() {
    // dst overlaying the addend, or an addend in another bank:
    // the fold cannot reread b_i, so the scratch park stays.
    let m = parse(&add32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x24),
        ]),
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "overlayed bytes keep the scratch fold:\n{asm}"
    );
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0xA0),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x34),
        ]),
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "cross-bank bytes keep the scratch fold:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0xA1, W\n    MOVWF 0x70\n    MOVF 0x2D, W\n    MOVWF 0x35\n    MOVF 0x70, W\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x70, W\n    ADDWF 0x35, F"),
        "byte 1 cross-bank carry chain:\n{asm}"
    );
}

#[test]
fn add32_reg_reg_in_place_dst_skips_preload() {
    // dst overlaying the augend: no preload is needed, the fold reads
    // b_i and accumulates in place.
    let m = parse(&add32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x2C),
        ]),
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no scratch carry park remains:\n{asm}"
    );
    assert!(
        asm.contains(
            "    MOVF 0x25, W\n    BTFSC STATUS, 0 ; C\n    INCFSZ 0x25, W\n    ADDWF 0x2D, F"
        ),
        "byte 1 in-place carry fold:\n{asm}"
    );
}

#[test]
fn add32_reg_reg_carry_simulates_boundaries() {
    // The park-free fold keeps the carry across 0x00 and 0xFF
    // addend bytes with carry in and out.
    let ir = &add32_reg_reg_module();
    let map = [
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x2C),
        ("main::b", 0x30),
        ("main::r", 0x34),
    ];
    // 0x0000FFFF + 1 = 0x00010000 (carry through bytes 0-2).
    let got = sim_run_bytes(
        ir,
        &map,
        &[
            (0x20, 0xFF),
            (0x21, 0xFF),
            (0x22, 0x00),
            (0x23, 0x00),
            (0x24, 0x01),
        ],
        0x28,
        4,
    );
    assert_eq!(got, vec![0x00, 0x00, 0x01, 0x00], "carry through bytes 0-2");
    // 0x0000FF80 + 0x00000080 = 0x00010000: byte 1's fold wraps
    // (0xFF + carry) and must pass the carry to byte 2.
    let got = sim_run_bytes(
        ir,
        &map,
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
    assert_eq!(got, vec![0x00, 0x00, 0x01, 0x00], "carry through the wrap");
    // 0x00050000 + 0x0003FF80 = 0x0008FF80: carry-in at the 0xFF byte
    // with a carry-out to byte 2.
    let got = sim_run_bytes(
        ir,
        &map,
        &[
            (0x20, 0x00),
            (0x21, 0x00),
            (0x22, 0x05),
            (0x23, 0x00),
            (0x24, 0x80),
            (0x25, 0xFF),
            (0x26, 0x03),
            (0x27, 0x00),
        ],
        0x28,
        4,
    );
    assert_eq!(
        got,
        vec![0x80, 0xFF, 0x08, 0x00],
        "carry in and out at the 0xFF byte"
    );
}

#[test]
fn sub32_and_sub_const_lhs_folds_simulate() {
    // epic-cc#1189/#1192: const borrow folds (including 0xFF bytes and
    // the const-LHS complement fold) keep the borrow across bytes.
    let map = [("x", 0x20), ("out", 0x24), ("main::r", 0x38)];
    // 0x01000000 - 0x0000FFFF = 0x00FF0001.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 %a, 65535\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0x00), (0x21, 0x00), (0x22, 0x00), (0x23, 0x01)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0x01, 0x00, 0xFF, 0x00],
        "sub32 borrow through 0xFF"
    );
    // 0x0000FF00 - 0x0000FFFF = 0xFFFFFF01: byte 1's 0xFF fold takes a
    // borrow in (a_0 = 0x00 under the 0xFF) and must pass it to byte 2.
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0x00), (0x21, 0xFF), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0x01, 0xFF, 0xFF, 0xFF],
        "sub32 borrow-in through 0xFF"
    );
    // 0x0000FF00 - 0x0000FF01 = 0xFFFFFFFF.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 65280, %a\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0x01), (0x21, 0xFF), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0xFF, 0xFF, 0xFF, 0xFF],
        "const-LHS borrow through 0xFF"
    );
    // 0x12345678 - 0x00000001 = 0x12345677 (no-borrow complement bytes).
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = sub i32 305419896, %a\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0x01), (0x21, 0x00), (0x22, 0x00), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0x77, 0x56, 0x34, 0x12],
        "const-LHS no-borrow bytes"
    );
}

fn sub32_reg_reg_module() -> String {
    "global x i32\nglobal y i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %b = load i32 @y\n    %r = sub i32 %a, %b\n    store i32 %r @out\n    ret void".to_string()
}

#[test]
fn sub32_reg_reg_borrow_folds_without_park() {
    // epic-cc#1224: the borrow folds onto b's own file byte via INCFSZ's
    // skip, so no scratch park is needed when dst leaves b addressable
    // in the same bank. All bytes here are bank-0 with no overlay.
    let m = parse(&sub32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x34),
        ]),
    );
    assert!(
        asm.contains("    MOVF 0x2D, W\n    MOVWF 0x35\n    MOVF 0x25, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x25, W\n    SUBWF 0x35, F"),
        "byte 1 park-free borrow fold:\n{asm}"
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no scratch borrow park remains:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF 0x70"),
        "no scratch park store remains:\n{asm}"
    );
}

#[test]
fn sub32_reg_reg_overlay_and_cross_bank_keep_park() {
    // dst overlaying the subtrahend, or a subtrahend in another bank:
    // the fold cannot reread b_i, so the scratch park stays.
    let m = parse(&sub32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x24),
        ]),
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "overlayed bytes keep the scratch fold:\n{asm}"
    );
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0xA0),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x34),
        ]),
    );
    assert_eq!(
        asm.matches("    INCFSZ 0x70, W").count(),
        3,
        "cross-bank bytes keep the scratch fold:\n{asm}"
    );
    assert!(
        asm.contains("    MOVF 0xA1, W\n    MOVWF 0x70\n    MOVF 0x2D, W\n    MOVWF 0x35\n    MOVF 0x70, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x70, W\n    SUBWF 0x35, F"),
        "byte 1 cross-bank borrow chain:\n{asm}"
    );
}

#[test]
fn sub32_reg_reg_in_place_dst_skips_preload() {
    // dst overlaying the minuend: no preload is needed, the fold reads
    // b_i and subtracts in place.
    let m = parse(&sub32_reg_reg_module());
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("y", 0x24),
            ("out", 0x28),
            ("main::a", 0x2C),
            ("main::b", 0x30),
            ("main::r", 0x2C),
        ]),
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no scratch borrow park remains:\n{asm}"
    );
    assert!(
        asm.contains(
            "    MOVF 0x25, W\n    BTFSS STATUS, 0 ; C\n    INCFSZ 0x25, W\n    SUBWF 0x2D, F"
        ),
        "byte 1 in-place borrow fold:\n{asm}"
    );
}

#[test]
fn sub32_reg_reg_borrow_simulates_boundaries() {
    // The park-free fold keeps the borrow across 0x00 and 0xFF
    // subtrahend bytes with borrow in and out.
    let ir = &sub32_reg_reg_module();
    let map = [
        ("x", 0x20),
        ("y", 0x24),
        ("out", 0x28),
        ("main::a", 0x2C),
        ("main::b", 0x30),
        ("main::r", 0x34),
    ];
    // 0x00010000 - 1 = 0x0000FFFF (borrow through bytes 0-2).
    let got = sim_run_bytes(
        ir,
        &map,
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
        got,
        vec![0xFF, 0xFF, 0x00, 0x00],
        "borrow through bytes 0-2"
    );
    // 0x0000FF00 - 0x0000FFFF = 0xFFFFFF01: byte 1's fold wraps
    // (0xFF + borrow) and must pass the borrow to byte 2.
    let got = sim_run_bytes(
        ir,
        &map,
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
    assert_eq!(got, vec![0x01, 0xFF, 0xFF, 0xFF], "borrow through the wrap");
    // 0x01050000 - 0x0103FF01 = 0x000100FF: borrow-in at the 0xFF byte
    // with a borrow-out to byte 2.
    let got = sim_run_bytes(
        ir,
        &map,
        &[
            (0x20, 0x00),
            (0x21, 0x00),
            (0x22, 0x05),
            (0x23, 0x01),
            (0x24, 0x01),
            (0x25, 0xFF),
            (0x26, 0x03),
            (0x27, 0x01),
        ],
        0x28,
        4,
    );
    assert_eq!(
        got,
        vec![0xFF, 0x00, 0x01, 0x00],
        "borrow in and out of 0xFF"
    );
    // 0x12345678 - 0x01010101 = 0x11335577 (no-borrow bytes stay exact).
    let got = sim_run_bytes(
        ir,
        &map,
        &[
            (0x20, 0x78),
            (0x21, 0x56),
            (0x22, 0x34),
            (0x23, 0x12),
            (0x24, 0x01),
            (0x25, 0x01),
            (0x26, 0x01),
            (0x27, 0x01),
        ],
        0x28,
        4,
    );
    assert_eq!(got, vec![0x77, 0x55, 0x33, 0x11], "no-borrow bytes");
}

#[test]
fn icmp_const_ff_byte_simulates() {
    // epic-cc#1192: the compare folds a 0xFF const byte in W as
    // a_i + 1 - borrow, keeping the borrow-out for the higher bytes.
    let ir = "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %c = icmp ult i32 %a, 16711680\n    store i8 %c @out\n    ret void\n";
    let map = [
        ("x", 0x20),
        ("out", 0x24),
        ("main::a", 0x28),
        ("main::c", 0x30),
    ];
    for (x, want) in [
        (0x00FE_FFFFu32, 1u8),
        (0x00FF_0000, 0),
        (0x0100_0000, 0),
        (0x0100_FF00, 0),
        (0x0000_0000, 1),
    ] {
        let seed: Vec<(u16, u8)> = (0..4)
            .map(|i| (0x20 + i as u16, (x >> (i * 8)) as u8))
            .collect();
        let got = sim_run_bytes(ir, &map, &seed, 0x24, 1);
        assert_eq!(got[0], want, "ult 0x00FF0000 with a = 0x{x:08X}");
    }
}
#[test]
fn add32_const_folds_simulate_carry_boundaries() {
    // epic-cc#1078/#1192: the ADDLW folds and the 0xFF fold must carry
    // exactly across 0x00 and 0xFF const bytes with carry in and out.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = add i32 %a, 65535\n    store i32 %r @out\n    ret void\n";
    let map = [("x", 0x20), ("out", 0x24), ("main::r", 0x38)];
    // 0x00FF0001 + 0x0000FFFF = 0x01000000: byte 1 wraps its 0xFF
    // fold with carry in and out, bytes 0 and 2 fold 0xFF/0x00.
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0x01), (0x21, 0x00), (0x22, 0xFF), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0x00, 0x00, 0x00, 0x01],
        "carry through 0xFF/0x00 folds"
    );
    // 0x00FF00FF + 0x01010101 = 0x02000200: every high byte folds
    // 0x01 with carry in, no wrap.
    let ir = "global x i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %a = load i32 @x\n    %r = add i32 %a, 16843009\n    store i32 %r @out\n    ret void\n";
    let got = sim_run_bytes(
        ir,
        &map,
        &[(0x20, 0xFF), (0x21, 0x00), (0x22, 0xFF), (0x23, 0x00)],
        0x24,
        4,
    );
    assert_eq!(
        got,
        vec![0x00, 0x02, 0x00, 0x02],
        "carry into every ADDLW fold"
    );
}

#[test]
fn icmp_ult_i16_const_folds_borrow_without_park() {
    // epic-cc#1078: the wide file-LHS borrow folds non-0xFF const bytes
    // into W the same way, so the SUBWF chain needs no scratch park.
    let m = parse(
        "global x i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %c = icmp ult i16 %a, 4660\n    store i8 %c @out\n    ret void\n",
    );
    // 4660 = 0x1234: byte 1 (0x12) folds. The icmp keeps `%a` staged
    // (loads feeding a compare are not single-use folds), so the fold
    // reads the slot at 0x24/0x25.
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("out", 0x22),
            ("main::a", 0x24),
            ("main::c", 0x26),
        ]),
    );
    assert!(
        asm.contains("    MOVLW 0x12\n    BTFSS STATUS, 0 ; C\n    ADDLW 0x01\n    SUBWF 0x25, W"),
        "byte 1 const borrow folds without a park:\n{asm}"
    );
    assert!(
        !asm.contains("INCFSZ 0x70, W"),
        "no scratch borrow park remains:\n{asm}"
    );
}

#[test]
fn icmp_eq_i16_zero_high_byte_threads_file_operand() {
    // epic-cc#1079: a skipped zero-XOR over a plain GPR load leaves the
    // byte addressable, so the running OR reads the file directly with
    // no park and no reload.
    let m = parse(
        "global x i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %c = icmp eq i16 %a, 65\n    store i8 %c @out\n    ret void\n",
    );
    // 65 = 0x0041: the high byte XORs zero and skips. `%a` stays
    // staged, so the OR threads slot 0x25 directly.
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("x", 0x20),
            ("out", 0x22),
            ("main::a", 0x24),
            ("main::c", 0x26),
        ]),
    );
    assert!(
        asm.contains("    IORWF 0x25, W"),
        "the high byte ORs its file slot directly:\n{asm}"
    );
    assert!(
        !asm.contains("XORLW 0x00"),
        "the zero high-byte XOR is skipped:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF 0x70"),
        "no scratch park remains:\n{asm}"
    );
}

#[test]
fn icmp_ne_i16_folds_simulate() {
    // epic-cc#1079: the park-free OR/XOR folds must report equality
    // exactly, including a high-byte-only difference (314 vs 58).
    let ir = "global x i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    %a = load i16 @x\n    %c = icmp ne i16 %a, 58\n    store i8 %c @out\n    ret void\n";
    let map = [
        ("x", 0x20),
        ("out", 0x22),
        ("main::a", 0x24),
        ("main::c", 0x26),
    ];
    for (lo, hi, want) in [
        (0x00u8, 0x00u8, 1),
        (0x3Au8, 0x00u8, 0),
        (0x3Au8, 0x01u8, 1),
    ] {
        let got = sim_run_bytes(ir, &map, &[(0x20, lo), (0x21, hi)], 0x22, 1);
        assert_eq!(got[0], want, "ne 58 with a = 0x{hi:02X}{lo:02X}");
    }
}
#[test]
fn direct_load_feeds_alu_from_the_source_global() {
    // `%1 = load @in` feeds one `add` operand: the op reads `@in`, so
    // `%1` stages nowhere. The single-byte `%2` computes into `@out`
    // (epic-cc#862), so it takes no address entry either.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    ADDLW 0x01\n    MOVWF 0x21"),
        "the add reads the source global into the store:\n{asm}"
    );
}

#[test]
fn fanout_load_threads_w_through_both_stores() {
    // `%1 = load @in; store %1 @slot; store %1 @out`: one read into W,
    // then a MOVWF per store. `%1` needs no entry: a lookup would panic,
    // so its absence proves the slot is gone.
    let m = parse("global in i8\nglobal slot i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @slot\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("slot", 0x21), ("out", 0x22)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x21\n    MOVWF 0x22"),
        "one read threading W through both stores:\n{asm}"
    );
    assert_eq!(
        asm.matches("MOVF 0x20, W").count(),
        1,
        "the source reads exactly once:\n{asm}"
    );
}

#[test]
fn forwarded_load_store_reads_the_source_global() {
    // `%1 = load @in; store %1 @slot`: the store reads `@in`, so the
    // copy never stages.
    let m = parse("global in i8\nglobal slot i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @slot\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("slot", 0x21)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x21"),
        "store reads the source global:\n{asm}"
    );
}

#[test]
fn forwarded_bin_result_computes_into_the_store() {
    // `%2 = add` into `store %2 @out`: the single-byte bin computes
    // straight into `@out` (epic-cc#862), so `%2` takes no address
    // entry: a lookup would panic, so its absence proves the slot is
    // gone.
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x20), ("out", 0x21)]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    ADDLW 0x01\n    MOVWF 0x21"),
        "the bin computes into the store destination:\n{asm}"
    );
}

#[test]
fn or_operand_load_stays_staged() {
    // `Or` has no fold shape here: the load stages even single-use, so
    // its slot entry stays live.
    let m = parse("global f i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @f\n    %2 = or i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let addrs = addrs(&[
        ("f", 0x20),
        ("out", 0x21),
        ("main::1", 0x25),
        ("main::2", 0x26),
    ]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x25"),
        "the or operand stages:\n{asm}"
    );
}

#[test]
fn multi_use_load_keeps_its_slot() {
    // `%1` feeds two binops: no fold applies and the slot stays.
    let m = parse("global in i8\nglobal out i8\nglobal out2 i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    %3 = add i8 %1, 2\n    store i8 %2 @out\n    store i8 %3 @out2\n    ret void\n");
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("out2", 0x22),
        ("main::1", 0x25),
        ("main::2", 0x26),
        ("main::3", 0x27),
    ]);
    let asm = select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("MOVF 0x20, W\n    MOVWF 0x25"),
        "the shared load stages:\n{asm}"
    );
}
