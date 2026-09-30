//! Issue #4 acceptance: runtime-length memcpy. Two `llvm.memcpy` calls with
//! i16 register lengths (`i16 %4`, `i16 %5`) — the counted-loop path — plus
//! the loop's zero-length guard and the constant-index reads after the copy.
//!
//! Hand computation for in == 0x0A (10):
//!   k  = in & 0xFF = 10      -> memcpy(buf1, buf2, 10): buf1[i] = buf2[i]
//!   k2 = (in >> 4) = 0       -> memcpy(buf3, buf2, 0): the guard skips the
//!                               loop, buf3 stays all zeros
//!   out = buf1[9] + buf3[4]  = (9*0x37 & 0xFF) + 0x00 = 0xEF + 0x00 = 0xEF
//!
//! The 16-byte spans of buf1/buf2/buf3 fit one FSR window (0x20..0x80), so
//! the per-byte FSR recomputes stay window-legal at every index.

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
fn dynamic_length_memcpy_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/dynamic_memcpy.c",
            "-o",
            "tests/fixtures/dynamic_memcpy.hex",
            "--map",
            "tests/fixtures/dynamic_memcpy.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/dynamic_memcpy.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/dynamic_memcpy.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/dynamic_memcpy.map");
    let in_addr = map_addr(&map, "in");
    let out_addr = map_addr(&map, "out");
    let buf1 = map_addr(&map, "buf1");
    let buf2 = map_addr(&map, "buf2");
    let buf3 = map_addr(&map, "buf3");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    // RAM globals are not initialized by the pipeline (the simulator starts
    // zeroed, like float_e2e's `in`); seed buf2's pattern — the copy source.
    for i in 0..16 {
        p.ram_mut()[buf2 + i] = (i as u8).wrapping_mul(0x37);
    }
    p.ram_mut()[in_addr] = 0x0A; // in = 10
    p.ram_mut()[in_addr + 1] = 0x00;
    p.run(2_000_000);

    assert_eq!(
        p.ram()[out_addr],
        0xEF,
        "out == 0xEF for in == 10 (10-byte runtime copy + zero-length guard)"
    );
    // buf1[0..9] = the pattern (10 bytes copied)
    for i in 0..10 {
        assert_eq!(p.ram()[buf1 + i], (i as u8).wrapping_mul(0x37), "buf1[{i}]");
    }
    assert_eq!(
        p.ram()[buf1 + 10],
        0,
        "buf1[10] untouched (only 10 bytes copied)"
    );
    // buf3 untouched by the zero-length copy
    for i in 0..8 {
        assert_eq!(p.ram()[buf3 + i], 0, "buf3[{i}] stays zero (len-0 guard)");
    }
    assert!(p.halted());
}
