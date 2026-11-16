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
