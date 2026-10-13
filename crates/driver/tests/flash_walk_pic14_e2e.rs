//! Flash-address walks on the PIC14 family (epic-cc#1197, epic-cc#1203).
//! A pointer phi over a flash string used to hold the string's flash
//! address while every load through it went through INDF (RAM): the basic
//! walk wrote 'E' (0x45, the first byte) instead of the trailing '\n'. The
//! family has no variable-index flash reader, so a const that reaches a
//! pointer phi now keeps a RAM copy (up to 255 bytes) and the walk reads
//! that. Over 255 bytes there is no copy and the backends refuse loudly.
//! A stored flash address of a small const keeps the same kind of RAM copy
//! (epic-cc#1223), so the slot holds a RAM address. The baseline core
//! keeps no RAM copy and refuses at slot lookup. Shapes that never hold a
//! flash address in a slot read through the RETLW reader and stay correct.

use std::process::Command;

const FLASH_CORES: [&str; 2] = ["p16f877a", "p16f1937"];

fn driver(device: &str, fixture: &str, tag: &str) -> (std::process::Output, std::path::PathBuf) {
    let stem = std::path::Path::new(fixture)
        .file_stem()
        .unwrap()
        .to_string_lossy();
    let hex_path = std::env::temp_dir().join(format!(
        "flash_walk_pic14_{tag}_{stem}_{device}_{}.hex",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device, "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(hex_path.with_extension("map"))
        .arg(fixture)
        .output()
        .expect("run driver");
    (out, hex_path)
}

fn expect_refusal(device: &str, fixture: &str, needle: &str) {
    let (out, hex_path) = driver(device, fixture, "refused");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(hex_path.with_extension("map"));
    assert!(
        !out.status.success(),
        "{device} {fixture} must not compile: a flash address read as RAM is a silent miscompile"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(needle),
        "{device} {fixture}: the error must contain {needle:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Compiles `fixture` for `device`, runs it on the simulator, and returns
/// the value of each named global.
fn run(device: &str, fixture: &str, names: &[&str]) -> Vec<u8> {
    let (out, hex_path) = driver(device, fixture, "run");
    assert!(
        out.status.success(),
        "{device} {fixture}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(hex_path.with_extension("map")).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(hex_path.with_extension("map"));
    let (ram, halted) = match device {
        "p16f877a" => {
            let mut p =
                pic14_sim::Pic14::with_device(&device::PIC16F877A, pic14_sim::parse_hex(&hex));
            p.run(3_000_000);
            (p.ram().to_vec(), p.halted())
        }
        "p16f1937" => {
            let mut p = pic14_sim::Pic14e::with_device(
                &device::PIC16F1937,
                pic14_sim::parse_hex_pic14e(&hex),
            );
            p.run(3_000_000);
            (p.ram().to_vec(), p.halted())
        }
        "p12f509" => {
            let mut p =
                pic14_sim::PicBaseline::with_device(&device::PIC12F509, pic14_sim::parse_hex(&hex));
            p.run(3_000_000);
            (p.ram().to_vec(), p.halted())
        }
        other => panic!("no simulator wired for {other}"),
    };
    assert!(halted, "{device} {fixture} must halt");
    names.iter().map(|n| ram[map_addr(&map, n)]).collect()
}

const S: &[u8] = b"EPIC_HARNESS_RESULT: PASS\n";

fn sum_of(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

/// The ticket repro: the walk visits all 26 bytes and the last store is the
/// trailing '\n' (the miscompile left 0x45 in `out`).
#[test]
fn flash_string_walk_reads_the_literal_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        let v = run(device, "tests/fixtures/flash_walk.c", &["out", "sent"]);
        assert_eq!(v, [b'\n', 26], "{device}: out, sent");
    }
}

/// A walk read through a constant-offset gep (`q = p + 1; q[1]`).
#[test]
fn flash_walk_through_offset_gep_is_correct_on_pic14_family() {
    let expected = sum_of(&S[2..]);
    for device in ["p16f877a", "p16f1937", "p12f509"] {
        let v = run(device, "tests/fixtures/flash_walk_offset.c", &["sum", "n"]);
        assert_eq!(v, [expected, 25], "{device}: sum of q[1] over the walk");
    }
}

/// A runtime index over a walk that advanced three steps: the 7th byte, 'A'.
#[test]
fn flash_walk_runtime_index_is_correct_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        let v = run(
            device,
            "tests/fixtures/flash_walk_runtime_index.c",
            &["out", "n"],
        );
        assert_eq!(v, [b'A', 3], "{device}: p[k] after 3 steps");
        let v = run(device, "tests/fixtures/flash_walk_two_index.c", &["out"]);
        assert_eq!(v, [b'A'], "{device}: q[b] with p + 3, a = 1, b = 2");
    }
}

/// A select between a flash string and its offset folds to one base.
#[test]
fn flash_walk_selected_arm_is_correct_on_pic14_family() {
    for device in ["p16f877a", "p16f1937", "p12f509"] {
        let v = run(device, "tests/fixtures/flash_walk_selected.c", &["out"]);
        assert_eq!(v, [b'P'], "{device}: q[0] is the second literal byte");
    }
}

/// Two runtime indices into a flash table read through the RETLW reader.
#[test]
fn flash_table_two_runtime_indices_are_correct_on_pic14_family() {
    for device in ["p16f877a", "p16f1937", "p12f509"] {
        let v = run(
            device,
            "tests/fixtures/flash_walk_two_index_chain.c",
            &["out"],
        );
        assert_eq!(v, [19], "{device}: TABLE[2][3] is flat byte 19");
    }
}

/// A plain `memcpy` from the walk: `buf[0]` is the walk's current byte on
/// every step, so `sum` is the sum of the whole literal.
#[test]
fn flash_walk_plain_memcpy_is_correct_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        let v = run(
            device,
            "tests/fixtures/flash_walk_plain_memcpy.c",
            &["sum", "sent"],
        );
        assert_eq!(v, [sum_of(S), 26], "{device}: sum, sent");
    }
}

/// A pointer joining the literal and a RAM buffer (`n` is never set, so
/// the loop does not run and `p` stays the literal): the first byte.
#[test]
fn flash_walk_joined_with_ram_reads_the_literal_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        for fixture in [
            "tests/fixtures/flash_walk_mixed_phi.c",
            "tests/fixtures/flash_walk_mixed_index.c",
            "tests/fixtures/flash_walk_mixed_memcpy.c",
        ] {
            let v = run(device, fixture, &["out"]);
            assert_eq!(
                v,
                [S[0]],
                "{device} {fixture}: p[0] is the literal's first byte"
            );
        }
    }
}

/// A string over 255 bytes has no RAM copy, but an index into it keeps
/// reading the chunked RETLW table: 303 bytes, the last one written is 'd'.
#[test]
fn large_flash_index_walk_is_correct_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        let v = run(
            device,
            "tests/fixtures/flash_walk_large.c",
            &["out", "sent"],
        );
        assert_eq!(v, [b'd', (303u32 & 0xFF) as u8], "{device}: out, sent");
    }
}

/// A pointer walk over a const too long to copy is refused on every core
/// that would otherwise read RAM at the flash address. The counter-index
/// shape is refused too: its counter-plus-gep form is what the NUL-walk
/// planner seeks, and the planner stays out for the same reason.
#[test]
fn large_flash_pointer_walk_is_refused_on_pic14_and_pic14e() {
    expect_refusal(
        "p16f877a",
        "tests/fixtures/flash_walk_large_pointer.c",
        "pointer access through flash walk",
    );
    expect_refusal(
        "p16f1937",
        "tests/fixtures/flash_walk_large_pointer.c",
        "pointer access through flash walk",
    );
    expect_refusal(
        "p16f877a",
        "tests/fixtures/flash_walk_counter_index.c",
        "pointer access through flash walk",
    );
    expect_refusal(
        "p16f1937",
        "tests/fixtures/flash_walk_counter_index.c",
        "pointer access through flash walk",
    );
}

/// Large-string swaps: a load inside the loop trips the walk refusal; with
/// every load after the loop the back-edge copy cycle is what is refused.
#[test]
fn large_flash_swap_is_refused_on_pic14() {
    expect_refusal(
        "p16f877a",
        "tests/fixtures/flash_walk_large_swap.c",
        "pointer access through flash walk",
    );
    expect_refusal(
        "p16f877a",
        "tests/fixtures/flash_walk_large_swap_late_load.c",
        "phi cycle over a flash address",
    );
}

/// The baseline core keeps no RAM copy of a const reaching a walk, so the
/// literal has no address and lowering stops at slot lookup.
#[test]
fn flash_string_walk_is_refused_on_pic_baseline() {
    expect_refusal(
        "p12f509",
        "tests/fixtures/flash_walk.c",
        "no address for @.str",
    );
}

/// Storing a small flash address into RAM keeps a RAM copy of the const, so
/// a later load through the slot reads the literal bytes (epic-cc#1223),
/// as on p18.
#[test]
fn storing_a_flash_address_reads_ram_copy_on_pic14_and_pic14e() {
    for device in FLASH_CORES {
        let v = run(
            device,
            "tests/fixtures/flash_walk_store_address_const.c",
            &["out"],
        );
        assert_eq!(v, [b'P'], "{device}: constant offset into the RAM copy");
        let v = run(
            device,
            "tests/fixtures/flash_walk_store_address_walk.c",
            &["out"],
        );
        assert_eq!(v, [b'C'], "{device}: walked pointer into the RAM copy");
        let v = run(
            device,
            "tests/fixtures/flash_walk_store_address_runtime.c",
            &["out"],
        );
        assert_eq!(v, [b'C'], "{device}: runtime offset into the RAM copy");
    }
}
