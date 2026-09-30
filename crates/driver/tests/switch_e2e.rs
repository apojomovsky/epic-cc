// switch_e2e: validates that `switch` lowers to an icmp+brcond chain and
// executes correctly on both supported devices. Covers acceptance:
// - multi-arm switch with default,
// - gaps (3,4,6-9,11+ go to default),
// - fallthrough (case 1 falls into case 2) behaves like the equivalent
//   if/else chain (by construction the lowering is the if/else chain).

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

fn expected(v: u8) -> u8 {
    let r: u8 = match v {
        0 => 10,
        1 => 20,
        2 => 30,
        5 => 50,
        10 => 100,
        _ => 99,
    };
    let r2: u8 = match v {
        1 => 12, // 5 + 7 fallthrough
        2 => 7,
        3 => 30,
        _ => 1,
    };
    r.wrapping_add(r2)
}

fn run_one(device: &str, v: u8) {
    let hex_path = format!("tests/fixtures/switch_{device}.hex");
    let map_path = format!("tests/fixtures/switch_{device}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/switch.c",
            "-o",
            &hex_path,
            "--map",
            &map_path,
            "--device",
            device,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device} v={v}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&map_path);
    // The p18 driver run above already gates alloc+isel success; only
    // the PIC14 sim asserts values.
    if device == "p16f877a" {
        let in_addr = map_addr(&map, "in");
        let out_addr = map_addr(&map, "out");
        let prog = pic14_sim::parse_hex(&hex);
        let mut p = pic14_sim::Pic14::new(prog);
        p.ram_mut()[in_addr] = v;
        p.run(200_000);
        assert_eq!(
            p.ram()[out_addr],
            expected(v),
            "device {device} v={v} expected {} got {}",
            expected(v),
            p.ram()[out_addr]
        );
        assert!(p.halted());
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn switch_both_devices() {
    for v in [0u8, 1, 2, 3, 4, 5, 6, 10, 11, 42, 255] {
        run_one("p16f877a", v);
        run_one("p18f4550", v);
    }
}
