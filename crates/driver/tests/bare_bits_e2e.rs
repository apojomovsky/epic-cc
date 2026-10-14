//! Bare bit acceptance (epic-cc#707): the `GIE = 1` tutorial spelling
//! through the whole pipeline on PIC14. The simulator pins the final SFR
//! state (INTCON, PORTB, TRISB) and the branched `out`; the emitted
//! assembly pins the single-instruction lowering (`BSF`/`BCF`) including
//! the bank-1 entity. PIC18 headers emit no bare names until its lane
//! settles the shared-name rule, so this stays PIC14-only.

use std::process::Command;

fn build_to(fixture: &str, device: &str, stem: &str, emit: &str) -> String {
    let path = format!("tests/fixtures/{stem}_{device}.{emit}");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o", &path, "--device", device, "--emit", emit])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&path).expect("read output");
    let _ = std::fs::remove_file(&path);
    text
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
fn bare_bits_drive_sfr_bits_on_pic14() {
    let hex_path = "tests/fixtures/bare_bits_p16f877a.hex";
    let map_path = "tests/fixtures/bare_bits_p16f877a.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/bare_bits.c",
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
    let hex = std::fs::read_to_string(hex_path).expect("read hex");
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(200_000);
    assert!(p.halted(), "bare bits program must halt");
    assert_eq!(p.ram()[0x0B], 0x80, "GIE sets INTCON bit 7");
    assert_eq!(p.ram()[0x06], 0x02, "PORTB keeps struct-set RB1 only");
    // Bit entities take no RAM byte and no map entry.
    assert!(
        !map.lines().any(|l| l.contains("GIE")),
        "GIE must not appear in the map:\n{map}"
    );
    assert_eq!(p.ram()[0x86], 0x02, "bank-1 TRISB1 entity sets TRISB");
    // out = 0 (branch on the cleared RB0) plus PORTB (0x02).
    assert_eq!(p.ram()[map_addr(&map, "out")], 0x02, "out pins the read");
}

#[test]
fn bare_bits_lower_to_single_bit_ops() {
    let asm = build_to("tests/fixtures/bare_bits.c", "p16f877a", "bare_bits", "asm");
    assert!(
        asm.contains("    BSF 0x0B, 7"),
        "GIE = 1 sets INTCON bit 7:\n{asm}"
    );
    assert!(
        asm.contains("    BSF 0x06, 0"),
        "RB0 = 1 sets PORTB bit 0:\n{asm}"
    );
    assert!(
        asm.contains("    BCF 0x06, 0"),
        "RB0 = 0 clears PORTB bit 0:\n{asm}"
    );
    assert!(
        asm.contains("    BTFSC 0x06, 0"),
        "the RB0 read tests PORTB bit 0:\n{asm}"
    );
    // The TRISB1 store (bank 1) must select its bank first, or the bit op
    // lands on PORTB again (the #112 shape, one bank up): the nearest
    // preceding RP0 op must be a set, never a clear.
    // PORTBbits.RB1 fuses to the same text; the TRISB1 store is the later one.
    let op = asm.rfind("    BSF 0x06, 1").expect("TRISB1 store");
    let before = &asm[..op];
    let set = before.rfind("    BSF STATUS, 5");
    let clr = before.rfind("    BCF STATUS, 5");
    assert!(
        set > clr,
        "the bank-1 select must precede the TRISB1 store:\n{asm}"
    );
}

#[test]
fn no_bit_defines_opt_out_builds_without_bare_names() {
    let path = "tests/fixtures/bare_bits_opt_out_p16f877a.hex";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/bank0_sfr.c",
            "-o",
            path,
            "--device",
            "p16f877a",
            "-D",
            "NO_BIT_DEFINES",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "opt-out build: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn address_of_a_bare_bit_is_rejected() {
    let hex_name = "tests/fixtures/bare_bits_reject_p16f877a.hex";
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/bare_bits_reject.c",
            "-o",
            hex_name,
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(hex_name);
    assert!(!output.status.success(), "&GIE must not compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot take the address of bit entity @GIE"),
        "names the rule: {stderr}"
    );
}

#[test]
fn struct_bit_rmw_fuses_to_single_bit_ops() {
    let asm = build_to(
        "tests/fixtures/struct_bit_rmw.c",
        "p16f877a",
        "struct_bit_rmw",
        "asm",
    );
    for op in [
        "    BSF 0x06, 0",
        "    BSF 0x06, 1",
        "    BCF 0x06, 0",
        "    BSF 0x06, 3",
        "    BSF 0x06, 4",
        "    BCF 0x06, 6",
        "    BCF 0x06, 7",
        "    BSF 0x06, 2",
        "    BSF 0x0B, 7",
    ] {
        assert!(asm.contains(op), "missing {op}:\n{asm}");
    }
    // The multi-bit 0xC0 mask keeps the byte read-modify-write.
    assert!(
        asm.contains("    IORLW 0xC0"),
        "0xC0 stays a byte RMW:\n{asm}"
    );
    // Every single-bit mask fused: no byte OR/AND of those masks remains.
    for lit in [
        "IORLW 0x02",
        "IORLW 0x08",
        "IORLW 0x10",
        "ANDLW 0xFE",
        "ANDLW 0xBF",
    ] {
        assert!(!asm.contains(lit), "{lit} must have fused:\n{asm}");
    }
}

#[test]
fn struct_bit_rmw_matches_the_byte_semantics_in_the_simulator() {
    let hex_path = "tests/fixtures/struct_bit_rmw_p16f877a.hex";
    let map_path = "tests/fixtures/struct_bit_rmw_p16f877a.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/struct_bit_rmw.c",
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
    let hex = std::fs::read_to_string(hex_path).expect("read hex");
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(hex_path);
    let _ = std::fs::remove_file(map_path);
    // The synthesized bit entities take no map entry.
    assert!(
        !map.lines().any(|l| l.contains("__epic_bit")),
        "bit entities must not appear in the map:\n{map}"
    );
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    assert_eq!(p.ram()[0x06], 0x1A, "PORTB after the struct writes");
    assert_eq!(p.ram()[0x86], 0x04, "TRISB2 via the bank-1 struct write");
    assert_eq!(p.ram()[0x0B], 0x80, "GIE via the INTCON struct write");
    assert_eq!(p.ram()[map_addr(&map, "out")], 0x1A, "out reads PORTB back");
}
