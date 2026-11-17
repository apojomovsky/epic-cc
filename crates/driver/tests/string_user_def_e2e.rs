//! A user definition of a `<string.h>` function must win over the
//! injected implementation instead of colliding with it at llvm-link,
//! while the remaining injected functions still link and run.

use std::process::Command;

fn run_p18(fixture: &str) -> (Vec<u8>, String, bool) {
    let hex_path = std::env::temp_dir().join(format!(
        "string_user_def_{}_{}.hex",
        std::process::id(),
        std::path::Path::new(fixture)
            .file_stem()
            .unwrap()
            .to_string_lossy()
    ));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver p18f4550: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let _ = std::fs::remove_file(&hex_path);

    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    p.run(200_000);
    (p.ram().to_vec(), map, p.halted())
}

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn user_memset_wins_and_injected_strcpy_still_runs() {
    let (ram, map, halted) = run_p18("tests/fixtures/string_user_memset.c");
    // User memset writes 10,11,12,13; injected strcpy then writes 'a','b',0
    // over the first three bytes, so buf[3] keeps the user's 13 and
    // strlen(buf) is 2: out is 15, not the injected memset's 10 + 2.
    assert_eq!(ram[map_addr(&map, "out")], 15);
    assert!(halted);
}

#[test]
fn user_strcpy_wins_and_injected_memset_still_runs() {
    let (ram, map, halted) = run_p18("tests/fixtures/string_user_strcpy.c");
    // Injected memset fills 'x'; user strcpy copies "abc" then bumps the
    // first byte to 'b', so out is 'b' + dst[4] ('x' left by memset).
    assert_eq!(ram[map_addr(&map, "out")], b'b' + b'x');
    assert!(halted);
}
