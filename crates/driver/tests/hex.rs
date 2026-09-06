//! The driver's final-stage hex emission (`driver::hex::emit`): program
//! words plus config bytes, per core. The pic14e arm is the D-3 fix: two
//! config words at byte 0x1000E need the `to_hex_regions` path PIC18
//! already uses, because the PIC14 arm's single-word splice cannot express
//! a second word and `to_hex`'s extended-linear-address record hard-codes
//! the upper 16 bits to zero.

use device::{Core, PIC16F1937, PIC16F877A};

/// Every data record as `(full byte address, bytes)`, resolving each
/// record's 16-bit address against the current `:04` extended-linear-
/// address record.
fn data_records(hex: &str) -> Vec<(u32, Vec<u8>)> {
    let mut upper: u32 = 0;
    let mut out = Vec::new();
    for line in hex.lines() {
        let rec = line.trim_start_matches(':');
        let count = u8::from_str_radix(&rec[0..2], 16).unwrap();
        let addr = u16::from_str_radix(&rec[2..6], 16).unwrap();
        match &rec[6..8] {
            "04" => upper = u32::from_str_radix(&rec[8..12], 16).unwrap() << 16,
            "00" => {
                let data = (0..count as usize)
                    .map(|i| u8::from_str_radix(&rec[8 + 2 * i..10 + 2 * i], 16).unwrap())
                    .collect::<Vec<_>>();
                out.push((upper + addr as u32, data));
            }
            _ => {}
        }
    }
    out
}

#[test]
fn pic14e_two_config_words_land_at_0x1000e_and_0x10010() {
    let dev = &PIC16F1937;
    assert_eq!(dev.core, Core::Pic14e);
    assert_eq!(dev.config.base_byte_addr, 0x1000E);
    assert_eq!(dev.config.num_bytes, 4);
    let cb = device::resolve_config(
        &dev.config,
        "osc=xt, wdt=off, pwrt=on, mclre=on, cp=off, cpd=off, boren=on, \
         clkouten=off, ieso=off, fcmen=off, wrt=off, vcapen=off, pllen=off, \
         stvren=on, borv=hi, lvp=off",
    );
    assert_eq!(cb.len(), 4);
    let program = vec![0x2830u16, 0x0064];
    let hex = driver::hex::emit(dev, &program, Some(&cb));
    let recs = data_records(&hex);
    // The config region is one 4-byte record at byte 0x1000E (CONFIG1 at
    // word 0x8007, CONFIG2 at word 0x8008), reached through a `:04`
    // extended-linear-address record for upper 0x0001.
    let cfg = recs
        .iter()
        .find(|(a, _)| *a == 0x1000E)
        .map(|(_, d)| d.clone());
    assert_eq!(cfg, Some(cb));
    // The program image is still present at base 0.
    assert!(recs.iter().any(|(a, d)| *a == 0 && d.len() == 4));
}

#[test]
fn pic14_config_word_lands_at_0x400e_within_the_program_image() {
    let dev = &PIC16F877A;
    let cb = device::resolve_config(
        &dev.config,
        "osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off, debug=off, cp=off",
    );
    assert_eq!(cb, vec![0x71, 0x3F]);
    let program = vec![0x2830u16, 0x0064];
    let hex = driver::hex::emit(dev, &program, Some(&cb));
    // The config word is spliced into the program image at byte 0x400E
    // (word 0x2007), the historical PIC14 shape: one image, no separate
    // config region.
    let recs = data_records(&hex);
    // The config word is spliced into the program image at byte 0x400E
    // (word 0x2007): the record starting at 0x4000 carries it at offset
    // 0x0E, and no separate config region exists.
    let containing = recs
        .iter()
        .find(|(a, d)| *a <= 0x400E && *a + d.len() as u32 > 0x400E)
        .map(|(a, d)| (d[(0x400E - a) as usize], d[(0x400F - a) as usize]));
    assert_eq!(containing, Some((cb[0], cb[1])));
    assert!(
        !recs.iter().any(|(a, _)| *a >= 0x10000),
        "no config region: {recs:?}"
    );
}
