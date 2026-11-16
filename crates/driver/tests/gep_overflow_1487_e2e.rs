// epic-cc#1487: a port read writing through its buffer argument, then
// a backward decode over a struct-field buffer, stopped with
// `iselcore: gep offset overflow`. clang reassociates the decode's
// `buf[len - 1]` into `(rx + len) - 1` once decode inlines across
// units, and the resolver summed the -1 (its u16 pattern 65535) with
// checked arithmetic. The three fixture units reproduce that chain;
// the sim proves the bytes the port wrote are the bytes read back.
//
// Hand computation (seed 0, so max == 7): port_read fills rx with
// 1..7, decode accepts (trailing byte 7, no zero, first code 7 covers
// the frame), shifts rx[0..6] one place up and reports out_len 6, so
// poll returns rx[1] == 1 and g_done == 1 ^ 0xA5 == 0xA4, halted.

use std::process::Command;

#[test]
fn gep_chain_through_negative_offset_runs_on_pic18() {
    let hex_path = "tests/fixtures/gep_overflow_1487_p18f4550.hex";
    let map_path = std::env::temp_dir().join(format!(
        "gep_overflow_1487_p18f4550-{}.map",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/gep_overflow_1487_port.c",
            "tests/fixtures/gep_overflow_1487_framer.c",
            "tests/fixtures/gep_overflow_1487_decode.c",
            "-I",
            "tests/fixtures",
            "-o",
            hex_path,
        ])
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
    let out_addr = at("g_out");
    let done_addr = at("g_done");

    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(500_000);
    assert!(p.halted(), "PIC18 must halt");

    let ram = p.ram();
    assert_eq!(ram[out_addr], 1, "g_out == rx[1] read back after decode");
    assert_eq!(ram[done_addr], 0xA4, "g_done == g_out ^ 0xA5");
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
}
