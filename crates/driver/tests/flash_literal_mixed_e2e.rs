//! A flash literal and a RAM buffer reach one non-inlined `const char *`
//! parameter (epic-cc#1109). A literal of 255 bytes or fewer is staged
//! into RAM, so the callee's INDF0 read returns the right byte for both
//! call sites. Pins that staged path on PIC18 under the simulator.
//!
//! out_flash: "EPIC_HARNESS_RESULT: PASS\n"[1] = 'P' = 0x50.
//! out_ram:   buf[2] = 'Q' = 0x51.

use std::process::Command;

/// RAM address of a global, read off the compiler's own `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn flash_literal_and_ram_buffer_share_callee() {
    let hex_path =
        std::env::temp_dir().join(format!("flash_literal_mixed_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/flash_literal_mixed.c")
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
    let out_flash = map_addr(&map, "out_flash");
    let out_ram = map_addr(&map, "out_ram");

    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    p.run(200_000);
    assert_eq!(p.ram()[out_flash], b'P', "flash literal byte");
    assert_eq!(p.ram()[out_ram], b'Q', "RAM buffer byte");
}

/// A flash literal over 255 bytes beside RAM actuals has no RAM copy to
/// serve the shared callee. The driver must reject it with a user error
/// that names the literal, not an internal compiler error.
#[test]
fn flash_literal_over_255_bytes_beside_ram_is_user_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(std::env::temp_dir().join(format!("flash_literal_long_{}.hex", std::process::id())))
        .arg("tests/fixtures/flash_literal_mixed_long.c")
        .output()
        .expect("run driver");
    assert!(
        !out.status.success(),
        "over-255-byte mixed literal must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("epic-cc: error: flash const") && stderr.contains("over the 255-byte"),
        "expected a user error naming the const, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("internal compiler error"),
        "must not be an ICE:\n{stderr}"
    );
}
