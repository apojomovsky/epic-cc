//! A flash address that passes through the integer domain keeps its space
//! (epic-cc#1346). The table's address is copied into a RAM pointer slot,
//! read back as an integer, stored in a global, and converted back to a
//! pointer. The dereference must read program memory: the RAM bytes at the
//! table's address hold no string, so a RAM read would return something
//! else entirely.
//!
//! The walk visits all 26 bytes of the literal, so `sent` pins the trip
//! count and `out` pins the last byte.

use std::process::Command;

/// Compiles `fixture` for p18f4550 and runs it on the simulator. Returns the
/// RAM image, the `--map` text, and whether the program halted.
fn run_p18(fixture: &str) -> (Vec<u8>, String, bool) {
    let hex_path = std::env::temp_dir().join(format!(
        "int_laundered_{}_{}.hex",
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
fn int_laundered_flash_address_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/int_laundered_flash.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        0x0a,
        "the laundered address must read the last table byte, not RAM"
    );
    assert_eq!(
        ram[map_addr(&map, "sent")],
        26,
        "the walk must visit all 26 bytes of the table"
    );
    assert!(halted);
}
