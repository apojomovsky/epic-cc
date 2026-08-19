use device::PIC16F877A;
use isel::select;
use ir::parse;

#[test]
fn dump_mul_asm() {
    let ir = "global ina float\n\
         global inb float\n\
         global out float\n\
         fn __mul_f32(float) (a=i32, b=i32)\n\
           block entry:\n\
             %__scr = alloca 14\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load float @ina\n\
             %y = load float @inb\n\
             %r = call float @__mul_f32(float %x, float %y)\n\
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
    map.push(("__mul_f32::a".to_string(), 0x40));
    map.push(("__mul_f32::b".to_string(), 0x44));
    map.push(("__mul_f32::__scr".to_string(), 0x48));
    let m = parse(ir);
    let asm = select(&PIC16F877A, &m, &map.iter().cloned().collect());
    // print from __mul_f32 onwards
    if let Some(idx) = asm.find("__mul_f32:") {
        println!("{}", &asm[idx..]);
    } else {
        println!("{asm}");
    }
}
