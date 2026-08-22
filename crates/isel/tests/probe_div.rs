use device::PIC16F877A;
use ir::parse;
use isel::select;

fn run_div(a: u32, b: u32) -> u32 {
    let ir = "global ina float\n\
         global inb float\n\
         global out float\n\
         fn __div_f32(float) (a=i32, b=i32)\n\
           block entry:\n\
             %__scr = alloca 14\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load float @ina\n\
             %y = load float @inb\n\
             %r = call float @__div_f32(float %x, float %y)\n\
             store float %r @out\n\
             ret void\n";
    let mut map = vec![
        ("ina".to_string(), 0x20u16),
        ("inb".to_string(), 0x24),
        ("out".to_string(), 0x28),
        ("main::x".to_string(), 0x2C),
        ("main::y".to_string(), 0x30),
        ("main::r".to_string(), 0x34),
    ];
    map.push(("__div_f32::a".to_string(), 0x40));
    map.push(("__div_f32::b".to_string(), 0x44));
    map.push(("__div_f32::__scr".to_string(), 0x48));
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &map.iter().cloned().collect());
    let words = asm::assemble(&asm);
    let mut p = pic14_sim::Pic14::new(words);
    p.ram_mut()[0x20..0x24].copy_from_slice(&a.to_le_bytes());
    p.ram_mut()[0x24..0x28].copy_from_slice(&b.to_le_bytes());
    p.run(200_000);
    assert!(p.halted(), "must halt");
    u32::from_le_bytes(p.ram()[0x28..0x2C].try_into().unwrap())
}

#[test]
fn probe() {
    let cases = [
        (0x3F80_0000u32, 0x4000_0000u32, "1.0 / 2.0"),
        (0x3F00_0000, 0x3F80_0000, "0.5 / 1.0"),
        (0x3F80_0000, 0x3F00_0000, "1.0 / 0.5"),
        (0x4000_0000, 0x3F80_0000, "2.0 / 1.0"),
        (0x4000_0000, 0x4000_0000, "2.0 / 2.0"),
        (0x3F80_0000, 0x3F80_0000, "1.0 / 1.0"),
        (0x3F80_0000, 0x4040_0000, "1.0 / 3.0"),
        (0x3F80_0000, 0x3F80_0001, "1.0 / (1+eps)"),
        (0x007F_FFFF, 0x4000_0000, "maxden / 2.0"),
        (0x0000_0001, 0x4000_0000, "minden / 2.0"),
        (0x007F_FFFF, 0x3F80_0000, "maxden / 1.0"),
        (0x007F_FFFF, 0x3F00_0000, "maxden / 0.5"),
        (0x3F80_0000, 0x7F80_0000, "1.0 / inf"),
        (0x7F80_0000, 0x4000_0000, "inf / 2.0"),
        (0x0000_0000, 0x0000_0000, "0 / 0"),
        (0x3F80_0000, 0x7FC0_0000, "1.0 / nan"),
        (0x7F80_0000, 0x7F80_0000, "inf / inf"),
        (0x3F80_0000, 0x0000_0000, "1.0 / 0"),
        (0x3F80_0000, 0x8000_0000, "1.0 / -0"),
        (0x7F7F_FFFF, 0x3F80_0000, "maxnormal / 1.0"),
        (0x3F80_0000, 0x7F7F_FFFF, "1.0 / maxnormal"),
        (0x7F80_0000, 0x007F_FFFF, "inf / maxden"),
        (0x007F_FFFF, 0x7F80_0000, "maxden / inf"),
        (0x4000_0000, 0x3F00_0000, "2.0 / 0.5"),
        (0x3F00_0000, 0x3F00_0000, "0.5 / 0.5"),
    ];
    for (a, b, label) in cases {
        let got = run_div(a, b);
        println!("{label}: {got:08X}");
    }
}
