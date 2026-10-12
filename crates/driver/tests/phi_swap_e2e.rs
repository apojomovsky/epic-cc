//! Pointer-phi swap cycles on the PIC14 family (epic-cc#1141): two walk
//! phis swap each iteration with one advancing through a gep over the
//! other, so the back-edge copies form a true cycle. Each backend stages
//! one arm through the first two retval bytes (dead at every block edge,
//! ISR-saved) and replays it once its destination is free. Flash string
//! literals stay p18-only on this family (variable-pointer loads read RAM
//! at the flash address), so all three backends prove the cycle over two
//! RAM buffers instead.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn compile(device: &str, fixture: &str, tag: &str) -> (Vec<u8>, String) {
    let hex_path = std::env::temp_dir().join(format!("phi_swap_{tag}_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device} {fixture}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let _ = std::fs::remove_file(&hex_path);
    (hex.into_bytes(), map)
}

fn run_pic14(device: &'static device::Device, hex: &[u8]) -> (Vec<u8>, bool) {
    let hex = String::from_utf8(hex.to_vec()).unwrap();
    let mut p = pic14_sim::Pic14::with_device(device, pic14_sim::parse_hex(&hex));
    p.run(200_000);
    (p.ram().to_vec(), p.halted())
}

#[test]
fn phi_swap_ram_pointers_on_pic14() {
    let (hex, map) = compile("p16f877a", "tests/fixtures/ptr_swap_ram.c", "ram_pic14");
    let (ram, halted) = run_pic14(&device::PIC16F877A, &hex);
    assert_eq!(
        ram[map_addr(&map, "sum")],
        166,
        "sum over the RAM swap walk"
    );
    assert_eq!(
        ram[map_addr(&map, "out")],
        76,
        "b[0] after 6 swaps must be 'L'"
    );
    assert!(halted);
}

/// The flash-string swap has no RAM base on PIC14: its walk slots hold
/// flash addresses, which variable-pointer loads would read as RAM. The
/// backend refuses loudly instead of miscompiling (epic-cc#1141); p18
/// TBLRDs the same fixture.
#[test]
fn phi_swap_flash_strings_refused_on_pic14() {
    let hex_path =
        std::env::temp_dir().join(format!("phi_swap_refused_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p16f877a", "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/flash_walk_swap.c")
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex_path);
    assert!(!out.status.success(), "a flash walk must fail the build");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("phi cycle over a flash address"),
        "the error must name the flash cycle: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_pic14e(hex: &[u8]) -> (Vec<u8>, bool) {
    let hex = String::from_utf8(hex.to_vec()).unwrap();
    let mut p =
        pic14_sim::Pic14e::with_device(&device::PIC16F1937, pic14_sim::parse_hex_pic14e(&hex));
    p.run(200_000);
    (p.ram().to_vec(), p.halted())
}

#[test]
fn phi_swap_ram_pointers_on_pic14e() {
    let (hex, map) = compile("p16f1937", "tests/fixtures/ptr_swap_ram.c", "ram_pic14e");
    let (ram, halted) = run_pic14e(&hex);
    assert_eq!(
        ram[map_addr(&map, "sum")],
        166,
        "sum over the RAM swap walk"
    );
    assert_eq!(
        ram[map_addr(&map, "out")],
        76,
        "b[0] after 6 swaps must be 'L'"
    );
    assert!(halted);
}

#[test]
fn phi_swap_ram_pointers_on_pic_baseline() {
    let (hex, map) = compile("p12f509", "tests/fixtures/ptr_swap_ram.c", "ram_baseline");
    let hex = String::from_utf8(hex).unwrap();
    let mut p = pic14_sim::PicBaseline::with_device(&device::PIC12F509, pic14_sim::parse_hex(&hex));
    p.run(200_000);
    let ram = p.ram().to_vec();
    assert_eq!(
        ram[map_addr(&map, "sum")],
        166,
        "sum over the RAM swap walk"
    );
    assert_eq!(
        ram[map_addr(&map, "out")],
        76,
        "b[0] after 6 swaps must be 'L'"
    );
    assert!(p.halted());
}
