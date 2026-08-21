#[test]
fn probe_edge_cases() {
    use ir::parse;
    use isel::select;
    use std::collections::HashMap;
    fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }
    fn f32_le(x: f32) -> [u8; 4] {
        x.to_bits().to_le_bytes()
    }
    fn sim_run_bytes(
        ir_text: &str,
        map: &[(String, u16)],
        seed: &[(u16, u8)],
        out: u16,
        n: usize,
    ) -> Vec<u8> {
        use pic14_sim::Pic14;
        let m = parse(ir_text);
        let asm = select(&device::PIC16F877A, &m, &addrs(&map_refs(map)));
        let words = asm::assemble(&asm);
        let mut p = Pic14::new(words);
        for (a, v) in seed {
            p.ram_mut()[*a as usize] = *v;
        }
        p.run(200_000);
        assert!(p.halted(), "must halt:\n{asm}");
        (0..n).map(|i| p.ram()[out as usize + i]).collect()
    }
    fn map_refs(map: &[(String, u16)]) -> Vec<(&str, u16)> {
        map.iter().map(|(k, v)| (k.as_str(), *v)).collect()
    }
    fn float_routine_module(name: &str) -> (String, Vec<(String, u16)>) {
        let (ret, params, scr) = match name {
            "__add_f32" | "__sub_f32" | "__mul_f32" => {
                ("float", &[("a", "i32"), ("b", "i32")][..], 14)
            }
            "__div_f32" => ("float", &[("a", "i32"), ("b", "i32")][..], 12),
            "__cmp_f32" => ("i8", &[("a", "i32"), ("b", "i32")][..], 6),
            _ => panic!("unknown"),
        };
        let pstr = params
            .iter()
            .map(|(n, t)| format!("{n}={t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let ir = format!(
            "global ina float\nglobal inb float\nglobal out {ret}\n\
             fn {name}({ret}) ({pstr})\n  block entry:\n    %__scr = alloca {scr}\n\
             fn main(void) ()\n  block entry:\n    %x = load float @ina\n    %y = load float @inb\n\
             %r = call {ret} @{name}(float %x, float %y)\n    store {ret} %r @out\n    ret void\n"
        );
        let mut map = vec![
            ("ina".to_string(), 0x20u16),
            ("inb".to_string(), 0x24),
            ("out".to_string(), 0x28),
            ("main::x".to_string(), 0x2C),
            ("main::y".to_string(), 0x30),
            ("main::r".to_string(), 0x34),
        ];
        let mut base = 0x40u16;
        for (pn, _) in params {
            map.push((format!("{name}::{pn}"), base));
            base += 4;
        }
        map.push((format!("{name}::__scr"), base));
        (ir, map)
    }
    let cases: &[(&str, f32, f32)] = &[
        ("__add_f32", f32::NAN, 1.0),
        ("__add_f32", 1.0, f32::NAN),
        ("__add_f32", f32::INFINITY, 1.0),
        ("__add_f32", 1.0, f32::INFINITY),
        ("__add_f32", f32::INFINITY, f32::NEG_INFINITY),
        ("__add_f32", f32::from_bits(0x007FFFFF), 1.0), // max denormal
        (
            "__add_f32",
            f32::from_bits(0x00000001),
            f32::from_bits(0x00000001),
        ), // denormal+denormal
        ("__mul_f32", f32::NAN, 1.0),
        ("__mul_f32", f32::INFINITY, 2.0),
        ("__mul_f32", f32::INFINITY, 0.0),
        ("__mul_f32", f32::from_bits(0x007FFFFF), 2.0), // denormal * 2
        ("__div_f32", f32::NAN, 1.0),
        ("__div_f32", 1.0, f32::NAN),
        ("__div_f32", f32::INFINITY, 2.0),
        ("__div_f32", 1.0, f32::INFINITY),
        ("__div_f32", 0.0, 0.0),
        ("__div_f32", f32::from_bits(0x007FFFFF), 2.0), // denormal / 2
        ("__sub_f32", f32::INFINITY, f32::INFINITY),
        ("__sub_f32", f32::NAN, 1.0),
    ];
    for &(name, a, b) in cases {
        let (ir, map) = float_routine_module(name);
        let mut seed = Vec::new();
        for (i, by) in f32_le(a).iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            seed.push((0x24 + i as u16, *by));
        }
        let want = match name {
            "__add_f32" => f32_le(a + b),
            "__sub_f32" => f32_le(a - b),
            "__mul_f32" => f32_le(a * b),
            "__div_f32" => f32_le(a / b),
            _ => unreachable!(),
        };
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
        let got_f = f32::from_bits(u32::from_le_bytes(got.clone().try_into().unwrap()));
        let want_f = f32::from_bits(u32::from_le_bytes(want.clone().try_into().unwrap()));
        let ok = got == want || (got_f.is_nan() && want_f.is_nan());
        eprintln!(
            "{name}({a:?}, {b:?}): got {got:02X?} ({got_f:?}) want {want:02X?} ({want_f:?}) {}",
            if ok { "OK" } else { "MISMATCH" }
        );
    }
}
