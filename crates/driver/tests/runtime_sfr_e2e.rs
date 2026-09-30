//! epic-cc#117 acceptance: volatile SFR access through a runtime address,
//! mirroring the real pic16_irq.c shapes (epic-hal#67 item 2).
//!
//! The fixture exercises all three IR shapes the pinned clang -O1 emits for
//! `pir_reg_addr(d)`:
//!   1. standalone runtime `inttoptr` (read_offset / write_offset);
//!   2. pointer select over two literal inttoptrs (GetFlag/ClearFlag's
//!      `pir_is_pir2 ? PIR2 : PIR1`);
//!   3. pointer phi joining the select result and the INTCON literal
//!      (GetFlag's `in_intcon ? INTCON : addr` join).
//!
//!                                (1 + 0 + PIR1 + PIR1), out_clear=1
//!   irq = 2 (TMR1, PIR1):        GetFlag=1, Clear clears PIR1, out_flag=1
//!     (1 + 0 + PIR1(0) + PIR1(0)), out_clear=2
//!   irq = 4 (BCL, PIR2):         GetFlag=1, Clear clears PIR2, out_flag=1
//!     (1 + 0 + PIR1(0) + PIR1(0)), out_clear=0
//!   write_offset(0, 0xAA) writes PIR1 directly: out_write = 0xAA every run.
//! The map classifies the runtime address slots as ordinary RAM locals; the
//! table stays const (flash): `irq_table` has no RAM address.
//!                                (1 + 0 + PIR1 + PIR2), out_clear=1
//!   irq = 2 (TMR1, PIR1):        GetFlag=1, Clear clears PIR1, out_flag=3
//!     (1 + 0 + 0 + PIR2), out_clear=2
//!   irq = 4 (BCL, PIR2):         GetFlag=1, Clear clears PIR2, out_flag=1
//!     (1 + 0 + PIR1(0) + PIR2(0)), out_clear=0
//!   write_offset(0, 0xAA) writes PIR1 directly: out_write = 0xAA every run.
//! The map classifies the runtime address slots as ordinary RAM locals; the
//! table stays const (flash): `irq_table` has no RAM address.

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
fn runtime_sfr_shapes_read_and_write_the_right_address() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/runtime_sfr.c",
            "-o",
            "tests/fixtures/runtime_sfr.hex",
            "--map",
            "tests/fixtures/runtime_sfr.map",
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

    let hex = std::fs::read_to_string("tests/fixtures/runtime_sfr.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/runtime_sfr.map").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/runtime_sfr.map");
    let addr = |n: &str| map_addr(&map, n);

    // The table stays const (flash): no RAM allocation.
    assert!(
        !map.lines().any(|l| l.starts_with("global irq_table 0x")),
        "irq_table must stay in flash (no RAM address)"
    );

    let prog = pic14_sim::parse_hex(&hex);

    // irq = 0 (RB, INTCON): INTCON preloaded with RBIF; PIR1/PIR2 idle.
    // GetFlag reads INTCON (phi shape); ClearFlag clears RBIF in INTCON.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("irq")] = 0;
    p.ram_mut()[0x0B] = 0x08; // INTCON: RBIF set
    p.ram_mut()[0x0C] = 0x01; // PIR1 idle
    p.ram_mut()[0x0D] = 0x01; // PIR2 idle
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_flag")], 3, "flag sum irq=0");
    assert_eq!(p.ram()[addr("out_clear")], 0x01, "PIR1|PIR2 irq=0");
    assert_eq!(p.ram()[addr("out_write")], 0xAA, "direct PIR1 write irq=0");
    assert_eq!(p.ram()[0x0B], 0x00, "INTCON RBIF cleared irq=0");

    // irq = 2 (TMR1, PIR1): GetFlag/ClearFlag use the PIR1 literal arm.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("irq")] = 2;
    p.ram_mut()[0x0B] = 0x00;
    p.ram_mut()[0x0C] = 0x01; // PIR1: TMR1IF set
    p.ram_mut()[0x0D] = 0x02; // PIR2 idle (CCP2IF)
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_flag")], 1, "flag sum irq=2");
    assert_eq!(p.ram()[addr("out_clear")], 0x02, "PIR1|PIR2 irq=2");
    assert_eq!(p.ram()[addr("out_write")], 0xAA, "direct PIR1 write irq=2");
    assert_eq!(
        p.ram()[0x0C],
        0xAA,
        "PIR1 overwritten by write_offset irq=2"
    );

    // irq = 4 (BCL, PIR2): GetFlag/select use the PIR2 path.
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("irq")] = 4;
    p.ram_mut()[0x0B] = 0x00;
    p.ram_mut()[0x0C] = 0x00;
    p.ram_mut()[0x0D] = 0x01; // PIR2: BCLIF set
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_flag")], 1, "flag sum irq=4");
    assert_eq!(p.ram()[addr("out_clear")], 0x00, "PIR1|PIR2 irq=4");
    assert_eq!(p.ram()[addr("out_write")], 0xAA, "direct PIR1 write irq=4");
    assert_eq!(p.ram()[0x0D], 0x00, "PIR2 BCLIF cleared irq=4");
}
