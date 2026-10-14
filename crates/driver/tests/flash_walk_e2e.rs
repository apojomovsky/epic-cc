//! A `const char *` walk over a flash string with no call argument (#1091).
//! The walk pointer is a phi over `.str` and its own gep, so each load
//! through it must read program memory (TBLRD). Before the fix the loads
//! went through INDF0 at the flash address and the final byte came back as
//! the first character ('E', 0x45) instead of the terminating '\n' (0x0a).
//!
//! The walk runs 26 times (`EPIC_HARNESS_RESULT: PASS\n`), so `sent` pins
//! the trip count as well as the last byte. The offset fixture reads through
//! a constant-offset gep over the walk phi (`q = p + 1; q[1]`), whose result
//! is pinned against the literal bytes.
//!
//! The mixed-phi fixtures (epic-cc#1099) join a flash literal with a RAM buffer.
//! Direct and runtime-indexed loads and stores through them are refused at
//! compile time, never emitted as a read or write at a flash address.

use std::process::Command;

/// Compiles `fixture` for p18f4550 and runs it on the simulator. Returns the
/// RAM image, the `--map` text, and whether the program halted.
fn run_p18(fixture: &str) -> (Vec<u8>, String, bool) {
    let hex_path = std::env::temp_dir().join(format!(
        "flash_walk_{}_{}.hex",
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

/// Reads a global's RAM address off the compiler's `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn flash_string_walk_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        0x0a,
        "last walked byte must be '\\n'"
    );
    assert_eq!(
        ram[map_addr(&map, "sent")],
        26,
        "walk must visit all 26 bytes"
    );
    assert!(halted);
}

#[test]
fn flash_string_walk_through_offset_gep_reads_program_memory_on_p18() {
    const S: &[u8] = b"EPIC_HARNESS_RESULT: PASS\n";
    // Iteration i loads `p[2]` for i in 0..25, so the sum covers S[2..26].
    let expected_sum = S[2..].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_offset.c");
    assert_eq!(
        ram[map_addr(&map, "sum")],
        expected_sum,
        "sum of q[1] over the walk"
    );
    assert_eq!(ram[map_addr(&map, "n")], 25, "walk must take 25 steps");
    assert!(halted);
}

/// A runtime index over a flash walk phi reads program memory (epic-cc#1101):
/// `p` advances 3 times, then `p[3]` is the 7th byte of the literal ('A').
#[test]
fn flash_walk_runtime_index_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_runtime_index.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'A',
        "p[k] after 3 walk steps must read the literal, not INDF0 at flash"
    );
    assert!(halted);
}

/// A runtime condition selects between a walk arm and its offset (epic-cc#1101).
/// The folded select is one walk slot plus one runtime term, so `q[0]` reads
/// the literal at the selected index: 'P' for `c == 1`.
#[test]
fn flash_walk_selected_arm_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_selected.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'P',
        "q[0] with c == 1 must be the second literal byte"
    );
    assert!(halted);
}

/// A store through a runtime-indexed flash walk writes ROM: refuse it
/// (epic-cc#1101), never emit a RAM write at a flash address.
#[test]
fn flash_walk_runtime_index_store_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_runtime_index_store.c",
        "ROM is not writable",
    );
}

/// Two runtime indices stacked on a walk phi each add to TBLPTR (epic-cc#1116):
/// `p` advances 3 times, `q = p + 1`, so `q[2]` is the 7th byte of the literal ('A').
#[test]
fn flash_walk_two_runtime_indices_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_two_index.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'A',
        "q[b] must add both runtime terms"
    );
    assert!(halted);
}

/// Two runtime indices over one flash table take the TBLPTR shift-add chain
/// (epic-cc#1126). The row stride (8) makes the chain win on `a`, and the
/// skip path adds `b`: `TABLE[2][3]` is flat byte 19.
#[test]
fn flash_walk_two_runtime_indices_chain_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_two_index_chain.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        19,
        "chain term plus skipped term must land on TABLE[2][3]"
    );
    assert!(halted);
}

/// Storing a small ROM address into RAM keeps a RAM copy of the const, so a
/// later load through the slot reads the literal bytes (epic-cc#1223).
/// Covers a runtime index, a constant base, and a walked pointer.
#[test]
fn flash_walk_store_of_rom_address_reads_ram_copy_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_store_address_runtime.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'C',
        "runtime offset into the RAM copy"
    );
    assert!(halted);
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_store_address_const.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'P',
        "constant offset into the RAM copy"
    );
    assert!(halted);
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_store_address_walk.c");
    assert_eq!(
        ram[map_addr(&map, "out")],
        b'C',
        "walked pointer into the RAM copy"
    );
    assert!(halted);
}

/// A const over 255 bytes has no RAM copy: storing its address is still
/// refused (epic-cc#1223 boundary).
#[test]
fn flash_walk_store_of_large_rom_address_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_store_address_large.c",
        "store of a ROM address into RAM",
    );
}

/// Compiles `fixture` for p18f4550 and asserts the driver refuses it with a
/// diagnostic containing `needle`.
fn expect_p18_rejection(fixture: &str, needle: &str) {
    let stem = std::path::Path::new(fixture)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let hex_path = std::env::temp_dir().join(format!("{stem}_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&hex_path);
    assert!(!out.status.success(), "{fixture} must not compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(needle), "stderr:\n{stderr}");
}

/// A flash literal joined with a RAM buffer (`p = buf` in the walk loop) has
/// no single address space. The driver must refuse it (epic-cc#1099) rather
/// than emit an INDF0 read at a flash address.
#[test]
fn flash_walk_phi_joined_with_ram_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_mixed_phi.c",
        "maybe-flash address",
    );
}

/// A runtime index over the same phi joins flash and RAM, so the load is refused
/// as a maybe-flash address (epic-cc#1099).
#[test]
fn flash_walk_mixed_runtime_index_load_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_mixed_index.c",
        "maybe-flash address",
    );
}

/// Two `const char *` walk phis swap each iteration, one advancing through a
/// gep over the other (epic-cc#1117, epic-cc#1127). The back-edge copy cycle
/// stages one arm through PRODL/PRODH, so every iteration reads pre-edge
/// bytes.
#[test]
fn flash_walk_phi_swap_reads_program_memory_on_p18() {
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_swap.c");
    assert_eq!(ram[map_addr(&map, "sum")], 162, "sum over the swap walk");
    assert_eq!(
        ram[map_addr(&map, "out")],
        68,
        "b[0] after 6 swaps must be 'D'"
    );
    assert!(halted);
}

/// The store through the runtime-indexed mixed phi lands on the same slot as
/// the load above: refused, never written to RAM at a flash address (epic-cc#1099).
#[test]
fn flash_walk_mixed_runtime_index_store_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_mixed_store.c",
        "ROM is not writable",
    );
}

/// A constant-length `memcpy` from a flash walk must copy program memory
/// (epic-cc#1100). `sum` adds the first copied byte on every step, so it pins
/// each walk position, not only the last one.
#[test]
fn flash_walk_memcpy_reads_program_memory_on_p18() {
    const S: &[u8] = b"EPIC_HARNESS_RESULT: PASS\n";
    let expected = S.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_memcpy.c");
    assert_eq!(
        ram[map_addr(&map, "sum")],
        expected,
        "sum of buf[0] over the walk"
    );
    assert_eq!(
        ram[map_addr(&map, "sent")],
        26,
        "walk must visit all 26 bytes"
    );
    assert!(halted);
}

/// A `memcpy` from a pointer phi joining flash and RAM has no single address
/// space, so it is refused at compile time (epic-cc#1100, epic-cc#1099).
#[test]
fn flash_walk_mixed_memcpy_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_mixed_memcpy.c",
        "memcpy from maybe-flash address",
    );
}

/// The ticket's shape: a plain `memcpy` call (not `__builtin_memcpy`) from a
/// flash walk with a constant length must copy program memory (epic-cc#1100).
/// Before the redirect the libc body read RAM at the walk's flash address.
#[test]
fn flash_walk_plain_memcpy_reads_program_memory_on_p18() {
    const S: &[u8] = b"EPIC_HARNESS_RESULT: PASS\n";
    let expected = S.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let (ram, map, halted) = run_p18("tests/fixtures/flash_walk_plain_memcpy.c");
    assert_eq!(
        ram[map_addr(&map, "sum")],
        expected,
        "sum of buf[0] over the walk"
    );
    assert_eq!(
        ram[map_addr(&map, "sent")],
        26,
        "walk must visit all 26 bytes"
    );
    assert!(halted);
}

/// A plain `memcpy` from a flash walk with a runtime length has no inline
/// lowering, so it is refused at compile time (epic-cc#1100).
#[test]
fn flash_walk_plain_memcpy_runtime_length_is_rejected_on_p18() {
    expect_p18_rejection(
        "tests/fixtures/flash_walk_plain_memcpy_runtime.c",
        "memcpy from a flash pointer with a runtime length",
    );
}
