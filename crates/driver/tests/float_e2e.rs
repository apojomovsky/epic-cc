//! Milestone-15 soft-float acceptance: a straight-line program exercising
//! the whole float surface — an fdiv call through a noinline helper
//! (`half`), fadd, fmul, the fptosi + sitofp int round trip, the fcmp
//! `olt` predicate, the RNE case 1.0f/3.0f, and a struct with a float
//! member via sret (`mk`) and byval (`pick`) — compiles through the whole
//! driver pipeline and runs correctly in the simulator. Acceptance: for
//! `in == 3.0f` the hand-computed `out1 == 0x3F99999A`, `out2 ==
//! 0x41100000`, `out3 == 0x3EAAAAAB` (the RNE 1.0/3.0) and the machine
//! halts.
//!
//! `in`, `out1..out3` are globals; their addresses are read from the same
//! alloc layout the driver used (see the trace in fixtures/float.c).
//!
//! Hand computation from the exact emitted IR (fixtures/float.c through
//! clang -O1; `in` = 3.0f = 0x40400000; abridged — the volatile store
//! copies are elided; the legalized fcmp becomes `call i8 @__cmp_f32` +
//! the olt tree `icmp eq i8 %c0, 1`):
//!   main:
//!     %1 = load volatile float @in                  3.0f = 0x40400000
//!     %2 = tail call float @half(float %1)          3.0/2.5 = 1.2 (fdiv)
//!          store volatile float %2, @out1           0x3F99999A
//!     %3 = fadd float %1, 0.25                      3.0 + 0.25 = 3.25 (exact)
//!     %4 = fmul float %3, 3.0                       3.25 * 3.0 = 9.75 (exact)
//!     %5 = fptosi float %4 to i16                   9.75 -> 9 (truncate)
//!     %6 = sitofp i16 %5 to float                   9 -> 9.0f = 0x41100000
//!          store volatile float %6, @out2           0x41100000
//!     %7 = fcmp olt float %1, 0.75                  3.0 < 0.75 = false
//!     %8 = zext i1 %7 to i8                         c = 0
//!     %9 = fdiv float 1.0, %1                       1.0/3.0 = 0x3EAAAAAB (RNE)
//!     %10 = tail call float @struct_step(i8 %8, float %9)
//!          struct_step: s = mk(c, rt) via sret       s.c = 0, s.f = 0x3EAAAAAB
//!                       return pick(s) via byval     s.c ? 0.0f : s.f = 0x3EAAAAAB
//!          store volatile float %10, @out3          0x3EAAAAAB
//!   half:    %2 = fdiv float %0, 2.5   3.0/2.5 = 1.2: 1.2 = 1.001100110011001100110011...b
//!            RNE: the 24-bit mantissa truncates at bit 24 = 1 with sticky = 1,
//!            so it rounds UP: mantissa 0x19999A, exp 127 -> 0x3F99999A.
//!   The RNE case (the load-bearing one): 1.0/3.0 = 0.0101010101...b =
//!            1.0101010101...b x 2^-2. The 25th fraction bit (the round bit)
//!            is 1 with the sticky set (the 0101 pattern repeats forever), so
//!            RNE rounds UP: mantissa 0x2AAAAB, exp 125 -> 0x3EAAAAAB.
//!   (All values cross-checked bit-for-bit against Rust's f32 arithmetic —
//!    3.0f32/2.5f32 = 0x3F99999A, (3.0f32+0.25f32)*3.0f32 = 9.75, then the
//!    fptosi truncates 9.75 to 9, 9i16 as f32 = 0x41100000, 1.0f32/3.0f32 =
//!    0x3EAAAAAB.)

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

fn f32_bytes(x: f32) -> [u8; 4] {
    x.to_bits().to_le_bytes()
}

#[test]
fn float_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/float.c",
            "-o",
            "tests/fixtures/float.hex",
            "--map",
            "tests/fixtures/float.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/float.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/float.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/float.map");
    let in_addr = map_addr(&map, "in");
    let out1_addr = map_addr(&map, "out1");
    let out2_addr = map_addr(&map, "out2");
    let out3_addr = map_addr(&map, "out3");

    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    let val = f32_bytes(3.0); // in = 3.0 (little-endian f32)
    for i in 0..4 {
        p.ram_mut()[in_addr + i] = val[i];
    }
    p.run(5_000_000);

    let mut got1 = [0u8; 4];
    for i in 0..4 {
        got1[i] = p.ram()[out1_addr + i];
    }
    assert_eq!(
        got1,
        f32_bytes(3.0 / 2.5),
        "out1 == 3.0/2.5 = 1.2 = 0x3F99999A"
    );
    let mut got2 = [0u8; 4];
    for i in 0..4 {
        got2[i] = p.ram()[out2_addr + i];
    }
    assert_eq!(
        got2,
        f32_bytes(((3.0f32 + 0.25f32) * 3.0f32) as i16 as f32),
        "out2 == (float)(int)((3.0+0.25)*3.0) = 9.0 = 0x41100000"
    );
    let mut got3 = [0u8; 4];
    for i in 0..4 {
        got3[i] = p.ram()[out3_addr + i];
    }
    assert_eq!(
        got3,
        f32_bytes(1.0 / 3.0),
        "out3 == 1.0/3.0 = 0x3EAAAAAB (RNE)"
    );
    assert!(p.halted());
}
