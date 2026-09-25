// Gate A for epic-cc#679: an `extern`-declared incomplete array reads
// correctly on p16f877a through runtime indices. The reader TU only sees
// `extern const struct Item table[]` (lowered as `[0 x T]`); isel
// resolves its GEPs against the merged sized definition and reads the
// flash fields via the table readers. Expected: flags 165, values 1515,
// g_sum == 1680, g_done == ((1680 & 0xFF) ^ 0xA5) == 0x35, halted.

use std::process::Command;

fn mapped(map: &str, name: &str) -> usize {
    map.lines()
        .find_map(|l| {
            let mut parts = l.split_whitespace();
            (parts.next() == Some("global") && parts.next() == Some(name)).then(|| {
                usize::from_str_radix(parts.next().unwrap().trim_start_matches("0x"), 16).unwrap()
            })
        })
        .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
}

#[test]
fn extern_incomplete_array_reads_on_pic14() {
    let hex_path = "tests/fixtures/extern_array_p16f877a.hex";
    let map_path =
        std::env::temp_dir().join(format!("extern_array_p16f877a-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/extern_array_walk.c",
            "tests/fixtures/extern_array_defs.c",
            "tests/fixtures/extern_array_main.c",
            "-o",
            hex_path,
        ])
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver p16f877a: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let sum = mapped(&map, "g_sum");
    let done = mapped(&map, "g_done");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(500_000);
    assert!(p.halted(), "PIC14 must halt");
    let ram = p.ram();

    let got = ram[sum] as u16 | ((ram[sum + 1] as u16) << 8);
    assert_eq!(got, 1680, "g_sum");
    assert_eq!(ram[done], ((1680u16 ^ 0xA5u16) & 0xFF) as u8, "g_done");
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
}
