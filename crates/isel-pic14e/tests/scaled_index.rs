//! Scaled GEP index terms accumulate in W as a `MOVF %r,W` then
//! `ADDWF %r,W` chain, never through the scratch byte. Each test runs the
//! emitted program in the PIC16F1937 simulator, so a lost or extra
//! repetition addresses a neighbouring element and fails the byte check.

use device::PIC16F1937;
use ir::parse;
use isel_pic14e::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn run(asm: &str, seed: &[(u16, u8)]) -> pic14_sim::Pic14e {
    let words = asm::assemble_pic14e(asm);
    let mut p = pic14_sim::Pic14e::with_device(&PIC16F1937, words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    p
}

fn scalar(name: &str) -> ir::Global {
    ir::Global {
        name: name.into(),
        ty: ir::Ty::I8,
        is_const: false,
        size: 1,
        bytes: vec![0],
        addr: None,
        internal: false,
        refs: Vec::new(),
    }
}

#[test]
fn stride_five_ram_index_reads_each_record() {
    // `rec[i].field`: scale-5 term plus a constant field offset.
    let ir = "global in i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %p = gep @a +3 +5*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n";
    let asm = select(
        &PIC16F1937,
        &parse(ir),
        &addrs(&[
            ("in", 0x20),
            ("out", 0x21),
            ("a", 0x40),
            ("main::i", 0x29),
            ("main::v", 0x2A),
        ]),
    );
    assert!(
        asm.contains("MOVF 0x29, W\n    ADDWF 0x29, W\n    ADDWF 0x29, W\n    ADDWF 0x29, W\n    ADDWF 0x29, W\n    ADDLW 0x03\n    ADDWF FSR0L, F"),
        "scale-5 term is one load plus four adds in W:\n{asm}"
    );
    assert!(
        !asm.contains("0x70"),
        "the sum never parks in scratch:\n{asm}"
    );
    for i in 0u8..4 {
        let mut seed: Vec<(u16, u8)> = (0..20u16).map(|n| (0x40 + n, 0x80 + n as u8)).collect();
        seed.push((0x20, i));
        assert_eq!(
            run(&asm, &seed).ram()[0x21],
            0x80 + 3 + 5 * i,
            "a[3 + 5*{i}]"
        );
    }
}

#[test]
fn const_table_scaled_two_term_index_reads_each_element() {
    // The flash-table index W = k + 3*%i + 2*%j accumulates in W before the
    // reader CALL. Every (i, j) pair names a distinct table byte.
    let ir_text = "global in i8\nglobal jn i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %j = load i8 @jn\n    %p = gep @t +1 +3*%i +2*%j\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n";
    let mut m = parse(ir_text);
    m.globals = vec![
        scalar("in"),
        scalar("jn"),
        scalar("out"),
        ir::Global {
            name: "t".into(),
            ty: ir::Ty::I8,
            is_const: true,
            size: 24,
            bytes: (0..24).map(|n| 0x50 + n as u8).collect(),
            addr: None,
            internal: false,
            refs: Vec::new(),
        },
    ];
    let asm = select(
        &PIC16F1937,
        &m,
        &addrs(&[
            ("in", 0x20),
            ("jn", 0x21),
            ("out", 0x22),
            ("main::i", 0x29),
            ("main::j", 0x2A),
            ("main::v", 0x2B),
        ]),
    );
    assert!(
        asm.contains("MOVF 0x29, W\n    ADDWF 0x29, W\n    ADDWF 0x29, W\n    ADDWF 0x2A, W\n    ADDWF 0x2A, W\n    ADDLW 0x01\n    CALL __read_t"),
        "index accumulates in W, then the reader call:\n{asm}"
    );
    for i in 0u8..4 {
        for j in 0u8..4 {
            let p = run(&asm, &[(0x20, i), (0x21, j)]);
            assert_eq!(
                p.ram()[0x22],
                0x50 + 1 + 3 * i + 2 * j,
                "t[1 + 3*{i} + 2*{j}]"
            );
        }
    }
}

#[test]
fn sret_slot_with_scaled_index_stores_at_the_right_byte() {
    // The indirect FSR setup adds the slot's address to a scaled term:
    // FSR0 = [r] + 1 + 2*%i. Only the byte at target + 1 + 2*i changes.
    let ir = "global in i8\nfn make(void) (r=sret)\n  block entry:\n\
           %i = load i8 @in\n\
           %p = gep %r +1 +2*%i\n\
           store i8 18 %p\n\
           ret void\n\
         fn main(void) ()\n  block entry:\n\
           %buf = alloca 8\n\
           call void @make(sret %buf)\n\
           ret void\n";
    let asm = select(
        &PIC16F1937,
        &parse(ir),
        &addrs(&[
            ("in", 0x20),
            ("make::r", 0x25),
            ("make::i", 0x27),
            ("main::buf", 0x30),
        ]),
    );
    assert!(
        asm.contains("MOVF 0x27, W\n    ADDWF 0x27, W\n    ADDLW 0x01\n    ADDWF FSR0L, F"),
        "scaled sum accumulates in W before the FSR0 add:\n{asm}"
    );
    for i in 0u8..3 {
        let p = run(&asm, &[(0x20, i)]);
        for n in 0..8usize {
            let want = if n == 1 + 2 * i as usize { 18 } else { 0 };
            assert_eq!(p.ram()[0x30 + n], want, "buf[{n}] with i={i}");
        }
    }
}
