//! epic-cc#133 acceptance: a straight-line program with the pid clamp
//! pattern: signed min/max on i16 (folded by clang -O1 into
//! `llvm.smax`/`llvm.smin` intrinsic calls), then a 16x16 -> 32 signed
//! multiply (folded to `llvm.abs` intrinsics + a `mul i32`) feeding an
//! i16 truncate: compiles through the whole driver pipeline and runs
//! correctly in the simulator.
//!
//! `in_a`, `in_min`, `in_max`, `in_b`, `out` are globals; their addresses
//! come off the compiler's own `--map` output below.
//!
//! Hand computation (in_a = -3000, in_min = -1000, in_max = 1000,
//! in_b = 15), traced against the emitted IR in fixtures/pid_clamp.c:
//!   clamp(-3000, -1000, 1000) = -1000      (smax i16, then smin i16)
//!   mul_s16(-1000, 15): |a|,|b| abs -> 1000, 15; product = 15000;
//!     signs differ -> negate = -15000       (abs i16 x2 + mul nuw i32 + select)
//!   p >> 8 = -15000 >> 8 = -59 (arithmetic shift), trunc i32 -> i16 = -59
//!   out = -59 = 0xFFC5
//!
//! The mul i32 lowers to a `CALL __mul_u32` runtime routine (no hardware
//! multiply on PIC14), and the `llvm.abs` intrinsic's `i1 false` immarg
//! exercises the irparse call-arg fix.

use std::process::Command;

/// `in_a`, `in_min`, `in_max`, `in_b` and `out`'s RAM addresses, read off
/// the compiler's own `--map` output. Rebuilding the pipeline here instead
/// would be a second copy of `main.rs` that silently drifts: the PIC18 path
/// alone parses with switches preserved, and once the frames sit below the
/// globals a difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn pid_clamp_runs_correctly() {
    let hex_path = "tests/fixtures/pid_clamp.hex";
    let map_path = "tests/fixtures/pid_clamp.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/pid_clamp.c",
            "-o",
            hex_path,
            "--map",
            map_path,
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

    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(map_path);
    let a_addr = map_addr(&map, "in_a");
    let min_addr = map_addr(&map, "in_min");
    let max_addr = map_addr(&map, "in_max");
    let b_addr = map_addr(&map, "in_b");
    let out_addr = map_addr(&map, "out");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    let i16 = |v: i16| (v as u16).to_le_bytes();
    p.ram_mut()[a_addr..a_addr + 2].copy_from_slice(&i16(-3000));
    p.ram_mut()[min_addr..min_addr + 2].copy_from_slice(&i16(-1000));
    p.ram_mut()[max_addr..max_addr + 2].copy_from_slice(&i16(1000));
    p.ram_mut()[b_addr..b_addr + 2].copy_from_slice(&i16(15));
    p.run(500_000);
    let got = (p.ram()[out_addr] as u16) | ((p.ram()[out_addr + 1] as u16) << 8);
    assert_eq!(
        got as i16, -59,
        "out == hand-computed -59 for the clamp + s16 mul"
    );
    assert!(p.halted());
}
