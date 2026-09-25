// epic-cc#678: LSR pointer walks over pointer parameters run on p18f4550.
// Loop-reduce turns each `p[i]` walk into a pointer phi whose entry arm is
// a GEP over the param; iselcore must let the walk increment wait for phi
// seeding (not panic with "chain base missing"), and isel-pic18 must
// materialize the walk slot copy, including the scaled single-term move
// the stride-2 `unsigned short` walk needs. The seed stays volatile so
// the walks survive whole-program optimization; startup clears it to 0.
// The nonzero results (sums, sink, patterned bytes) prove the walks
// execute rather than fold away; the zeroed regions prove their bounds.
//
// Hand computation (seed 0, select 0, off2 0, n7 7):
//   g_buf[i] == i for i in 0..24, g_buf[24..32] == 0, g_buf2 all zero
//   g_s1 == 0+1+2+3+4+5 == 15
//   g_s2 == 1+4+7+10+13+16+19 == 70
//   g_done == ((15 + 70) & 0xFF) ^ 0xA5 == 0xF0, halted.

use std::process::Command;

#[test]
fn lsr_param_walks_run_on_pic18() {
    let hex_path = "tests/fixtures/lsr_param_walk_p18f4550.hex";
    let map_path = std::env::temp_dir().join(format!(
        "lsr_param_walk_p18f4550-{}.map",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/lsr_param_walk.c", "-o", hex_path])
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p18f4550"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver p18f4550: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();

    // Addresses come from the driver's own alloc map, so the test can
    // never skew from the compiled hex the way a re-derived layout can.
    let at = |name: &str| {
        map.lines()
            .find_map(|l| {
                let mut parts = l.split_whitespace();
                (parts.next() == Some("global") && parts.next() == Some(name)).then(|| {
                    usize::from_str_radix(parts.next().unwrap().trim_start_matches("0x"), 16)
                        .unwrap()
                })
            })
            .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
    };
    let buf = at("g_buf");
    let buf2 = at("g_buf2");
    let w = at("g_w");
    let w2 = at("g_w2");
    let s1 = at("g_s1");
    let s2 = at("g_s2");
    let done = at("g_done");

    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    // No seed preset: startup zero-initializes RAM, wiping any preset
    // before main reads it. Power-on seed 0 still proves the walks run:
    // every expectation below is nonzero.
    p.run(500_000);
    assert!(p.halted(), "PIC18 must halt");

    let ram = p.ram();
    let seed = 0u16;
    let u16_at = |a: usize| ram[a] as u16 | ((ram[a + 1] as u16) << 8);
    // The head of the buffer is intact: the zero walk neither underran
    // its target nor stayed dead.
    assert_eq!(ram[buf], seed as u8, "g_buf[0]");
    assert_eq!(ram[buf + 8], (seed as u8).wrapping_add(8), "g_buf[8]");
    assert_eq!(ram[buf + 23], (seed as u8).wrapping_add(23), "g_buf[23]");
    // The zero walk cleared exactly the tail of the buffer.
    for i in 24..32 {
        assert_eq!(ram[buf + i], 0, "g_buf[{i}] must be zeroed");
    }
    // The unselected twin buffers stayed zero: no walk strayed.
    for i in 0..40 {
        assert_eq!(ram[buf2 + i], 0, "g_buf2[{i}] must stay zero");
    }
    // The strided param read walked `start = seed & 7`, six bytes.
    let start = seed & 7;
    let want_s1 = 6 * seed + 6 * start + 15;
    assert_eq!(u16_at(s1), want_s1, "g_s1");
    // The stride-2 fill and sum walked the `unsigned short` array: the
    // sum covers the runtime window [off2, off2+n7).
    let off2 = seed & 1;
    let n7 = 7 - (seed & 1);
    let mut want_s2 = 0u16;
    for j in 0..n7 {
        want_s2 = want_s2.wrapping_add((((seed << 8) & 0xFFFF) | (3 * (off2 + j) + 1)) as u16);
    }
    assert_eq!(u16_at(s2), want_s2, "g_s2");
    for i in 0..8 {
        let want = ((seed << 8) & 0xFFFF) | (3 * i as u16 + 1);
        assert_eq!(u16_at(w + 2 * i), want, "g_w[{i}]");
        assert_eq!(u16_at(w2 + 2 * i), 0, "g_w2[{i}] must stay zero");
    }
    assert_eq!(
        ram[done],
        ((want_s1.wrapping_add(want_s2) & 0xFF) ^ 0xA5) as u8,
        "g_done"
    );
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
}
