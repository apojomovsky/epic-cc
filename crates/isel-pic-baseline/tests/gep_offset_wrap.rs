//! epic-cc#1466: a direct GEP whose static offset already passes the
//! baseline FSR width must refuse loudly instead of masking the
//! address onto a low bank byte.

use device::PIC12F509;
use ir::parse;
use isel_pic_baseline::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
#[should_panic(expected = "FSR range")]
fn gep_offset_past_fsr_width_is_rejected() {
    let m = parse(
        "global in i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %i = load i8 @in\n    %p = gep @a +200 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    let a = addrs(&[
        ("in", 0x10),
        ("a", 0x12),
        ("out", 0x13),
        ("main::i", 0x14),
        ("main::v", 0x15),
    ]);
    let _ = select(&PIC12F509, &m, &a);
}

#[test]
fn wrapped_negative_offset_in_range_is_accepted() {
    // @a = 0x12 with k = -1 (u16 pattern 0xFFFF): the wrapped sum is
    // 0x11, inside the PIC12F509 FSR range, so it must not refuse.
    let ir = "global in i8\nglobal a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %p = gep @a +65535 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n";
    let map = [
        ("in", 0x10u16),
        ("a", 0x12),
        ("out", 0x13),
        ("main::i", 0x14),
        ("main::v", 0x15),
    ];
    let asm = select(&PIC12F509, &parse(ir), &addrs(&map));
    for (i, want) in [(0u8, 0x5Au8), (1, 0x6B)] {
        let words = asm::assemble_pic_baseline(&asm);
        let mut p = pic14_sim::PicBaseline::with_device(&PIC12F509, words);
        p.ram_mut()[0x11] = 0x5A;
        p.ram_mut()[0x12] = 0x6B;
        p.ram_mut()[0x10] = i;
        p.run(10_000);
        assert!(p.halted(), "wrapped negative offset must halt:\n{asm}");
        assert_eq!(p.ram()[0x13], want, "a[-1 + {i}] reads the true cell");
    }
}
