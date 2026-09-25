// Gate B for epic-cc#679: clang's 2-bit deposit idiom (lowered via
// `llvm.bitreverse.i6`) deposits correctly on p16f877a. All four states
// go through the volatile pick so the idiom survives optimization.
// Expected: g_ports == {0x00, 0x20, 0x10, 0x30}, halted.

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
fn bit_deposit_idiom_runs_on_pic14() {
    let hex_path = "tests/fixtures/bitreverse_deposit_p16f877a.hex";
    let map_path = std::env::temp_dir().join(format!(
        "bitreverse_deposit_p16f877a-{}.map",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/bitreverse_deposit.c", "-o", hex_path])
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
    let ports = mapped(&map, "g_ports");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(500_000);
    assert!(p.halted(), "PIC14 must halt");

    let ram = p.ram();
    assert_eq!(
        [ram[ports], ram[ports + 1], ram[ports + 2], ram[ports + 3]],
        [0x00, 0x20, 0x10, 0x30],
        "g_ports"
    );
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
}
