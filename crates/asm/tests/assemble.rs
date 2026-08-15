use asm::{assemble, to_hex};
use pic14_sim::parse_hex;

#[test]
fn assembles_movf_add_movwf() {
    let src = "    org 0x0000\n    goto __start\n__start:\n    movf 0x20, W\n    movlw 0x01\n    addwf 0x20, W\n    movwf 0x21\n    sleep\n    end\n";
    let words = assemble(src);
    assert_eq!(words[0], 0x2801); // goto __start (word 1)
    assert_eq!(words[1], 0x0820); // movf 0x20, W
    assert_eq!(words[2], 0x3001); // movlw 0x01
    assert_eq!(words[3], 0x0720); // addwf 0x20, W
    assert_eq!(words[4], 0x00A1); // movwf 0x21
    assert_eq!(words[5], 0x0063); // sleep
}

#[test]
fn low_high_label_operands_resolve_via_symbol_table() {
    // Pass-2 literal resolution: `LOW(label)`/`HIGH(label)` operands resolve
    // through the symbol table (labels are word addresses): LOW = addr & 0xFF,
    // HIGH = (addr >> 8) & 0xFF. mytable sits at word 0x103: LOW = 0x03,
    // HIGH = 0x01.
    let src = "    org 0x0100\n    nop\n    nop\n    nop\nmytable:\n    addlw LOW(mytable)\n    addlw HIGH(mytable)\n    end\n";
    let words = assemble(src);
    // mytable sits at word 0x103 (org 0x100 + 3 nops).
    assert_eq!(words[0x103], 0x3E00 | 0x03, "ADDLW LOW(mytable)");
    assert_eq!(words[0x104], 0x3E00 | 0x01, "ADDLW HIGH(mytable)");
}

#[test]
#[should_panic(expected = "asm: file register 0x80 out of range")]
fn panics_on_file_register_out_of_range() {
    let src = "    org 0x0000\n    goto __start\n__start:\n    movwf 0x80\n    sleep\n    end\n";
    let _ = assemble(src);
}

#[test]
fn to_hex_roundtrips_through_parse_hex() {
    let src = "    org 0x0000\n    goto __start\n__start:\n    movf 0x20, W\n    movlw 0x01\n    addwf 0x20, W\n    movwf 0x21\n    sleep\n    end\n";
    let words = assemble(src);
    let hex = to_hex(&words);
    // Ends with 01 EOF record.
    assert!(hex.ends_with(":00000001FF\n"), "missing EOF: {hex:?}");
    // Byte order must match the simulator's decoder exactly.
    let decoded = parse_hex(&hex);
    assert_eq!(decoded[0], words[0]);
    assert_eq!(decoded[1], words[1]);
    assert_eq!(decoded[2], words[2]);
    assert_eq!(decoded[3], words[3]);
    assert_eq!(decoded[4], words[4]);
    assert_eq!(decoded[5], words[5]);
}

#[test]
fn to_hex_spans_records_and_roundtrips_large_program() {
    // >16 words forces multiple records; every record's byte-count/checksum
    // must survive parse_hex exactly.
    let mut src = String::from("    org 0x0000\n");
    for _ in 0..40 {
        src.push_str("    nop\n");
    }
    src.push_str("    sleep\n    end\n");
    let words = assemble(&src);
    assert_eq!(words.len(), 41);
    let hex = to_hex(&words);
    assert!(hex.lines().count() > 2, "expected multiple records");
    let decoded = parse_hex(&hex);
    for (i, w) in words.iter().enumerate() {
        assert_eq!(decoded[i], *w, "word {i} mismatch");
    }
}
