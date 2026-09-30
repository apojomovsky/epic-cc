//! Issue #148: string literal as pointer call argument must compile and read correctly.
use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn str_literal_compiles_to_hex_on_both_devices() {
    for (name, _dev) in [
        ("p16f877a", &device::PIC16F877A),
        ("p16f887", &device::PIC16F887),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args([
                "tests/fixtures/str_literal.c",
                "-o",
                &format!("tests/fixtures/str_literal_{name}.hex"),
                "--device",
                name,
            ])
            .output()
            .expect("run driver");
        assert!(
            out.status.success(),
            "driver for {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let hex =
            std::fs::read_to_string(format!("tests/fixtures/str_literal_{name}.hex")).unwrap();
        assert!(hex.contains(':'), "hex for {name} looks empty");
    }
}

#[test]
fn str_literal_bytes_are_readable_on_pic14() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/str_literal.c",
            "-o",
            "tests/fixtures/str_literal_run.hex",
            "--map",
            "tests/fixtures/str_literal_run.map",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string("tests/fixtures/str_literal_run.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/str_literal_run.map").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/str_literal_run.map");
    let g_tx = map_addr(&map, "g_tx");
    let g_tx_len = map_addr(&map, "g_tx_len");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(500_000);
    let expected = b"epic-serial ready\r\n";
    assert_eq!(p.ram()[g_tx_len], expected.len() as u8, "g_tx_len");
    for (i, b) in expected.iter().enumerate() {
        assert_eq!(p.ram()[g_tx + i], *b, "g_tx[{i}]");
    }
    assert!(p.halted());
}
