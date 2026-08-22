use asm::{assemble, assemble_file_to_hex, to_hex};
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
fn assembles_runtime_routine_instructions_with_destinations() {
    // The mul/div/rem routine recipes (isel M8 Task 3) use the F-destination
    // file ops and the carry/borrow loop instructions; the destination token
    // must select the d bit (W = 0, F = 1).
    let src = "    org 0x0000\n\
    rlf 0x20, F\n\
    rrf 0x21, F\n\
    incf 0x22, F\n\
    incfsz 0x23, W\n\
    decfsz 0x24, F\n\
    comf 0x25, F\n\
    addwf 0x26, F\n\
    subwf 0x27, W\n\
    end\n";
    let words = assemble(src);
    assert_eq!(words[0], 0x0DA0, "rlf 0x20, F");
    assert_eq!(words[1], 0x0CA1, "rrf 0x21, F");
    assert_eq!(words[2], 0x0AA2, "incf 0x22, F");
    assert_eq!(words[3], 0x0F23, "incfsz 0x23, W");
    assert_eq!(words[4], 0x0BA4, "decfsz 0x24, F");
    assert_eq!(words[5], 0x09A5, "comf 0x25, F");
    assert_eq!(words[6], 0x07A6, "addwf 0x26, F");
    assert_eq!(words[7], 0x0227, "subwf 0x27, W");
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

// ---- Milestone 13: SWAPF and RETFIE (the ISR save/restore path) ----

#[test]
fn assembles_swapf_and_retfie() {
    // The interrupt prologue/epilogue (isel M13 Task 2) nibble-swaps STATUS
    // through SWAPF (both directions: `SWAPF STATUS, W` to save without
    // touching STATUS, `SWAPF 0x76, W` to swap back) and ends with RETFIE
    // instead of RETURN. Both encodings are 14-bit core opcodes: SWAPF is
    // `0x0E00 | d<<7 | f` (d = 0 -> W, 1 -> F), RETFIE is `0x0009`.
    let src = "STATUS equ 0x03\n    org 0x0000\n\
    swapf STATUS, W\n\
    swapf 0x76, W\n\
    swapf 0x21, F\n\
    retfie\n\
    end\n";
    let words = assemble(src);
    assert_eq!(words[0], 0x0E03, "swapf STATUS, W (d = 0)");
    assert_eq!(words[1], 0x0E76, "swapf 0x76, W (d = 0)");
    assert_eq!(words[2], 0x0EA1, "swapf 0x21, F (d = 1)");
    assert_eq!(words[3], 0x0009, "retfie");
}

// ---- Milestone 11: the device-flash bound ----

#[test]
#[should_panic(expected = "exceeds device flash")]
fn panics_when_program_exceeds_device_flash() {
    // 0x4000 words fill the PIC16F877A's flash exactly (addresses
    // 0x000-0x3FFF); a program whose highest word address is ≥ 0x4000 (16K
    // words) cannot be stored and must panic loudly. `nop` is 0x0000 (to_hex
    // would trim it) but the bound assert fires before rendering.
    let src = String::from("    org 0x4000\n    nop\n    end\n");
    let _ = assemble_file_to_hex(&device::PIC16F877A, &src);
}

#[test]
fn device_flash_exact_fill_does_not_panic() {
    // Boundary: exactly 0x2000 words (highest word address 0x1FFF) fills the
    // device flash and must assemble to a full HEX image. `movlw 0x00`
    // (0x3000) is nonzero so to_hex does not trim the trailing words.
    let mut src = String::from("    org 0x0000\n");
    for _ in 0..0x2000 {
        src.push_str("    movlw 0x00\n");
    }
    src.push_str("    end\n");
    let hex = assemble_file_to_hex(&device::PIC16F877A, &src);
    assert!(hex.contains(":00000001FF\n"), "missing EOF record: {hex:?}");
    assert!(
        hex.lines().count() > 2,
        "full-flash image must span records"
    );
}

#[test]
fn program_at_word_0x1000_assembles() {
    // The M10 page-0 assert (highest word address < 0x800) must NOT fire for
    // a program located at 0x1000 — M11 replaces it with the device-flash
    // bound (< 0x2000). A single instruction placed at 0x1000 is legal now.
    let src = "    org 0x1000\n    movlw 0x2A\n    end\n";
    let words = assemble_file_to_hex(&device::PIC16F877A, &src);
    assert!(
        words.contains(":00000001FF\n"),
        "missing EOF record: {words:?}"
    );
}

// ---- Milestone 11: PAGE(label) resolution ----

#[test]
fn page_label_operands_resolve_via_symbol_table() {
    // `PAGE(label)` resolves through the symbol table to `(addr >> 11) << 3`
    // — the PCLATH<4:3> page bits (bits 2:0 clear). Labels placed at each
    // page base via `.org` give 0x00/0x08/0x10/0x18 for pages 0-3.
    let src = "    org 0x0000\np0:\n    movlw PAGE(p0)\n\
                org 0x0800\np1:\n    movlw PAGE(p1)\n\
                org 0x1000\np2:\n    movlw PAGE(p2)\n\
                org 0x1800\np3:\n    movlw PAGE(p3)\n    end\n";
    let words = assemble(src);
    assert_eq!(words[0x000], 0x3000 | 0x00, "PAGE(p0) at 0x000 -> page 0");
    assert_eq!(words[0x800], 0x3000 | 0x08, "PAGE(p1) at 0x800 -> page 1");
    assert_eq!(words[0x1000], 0x3000 | 0x10, "PAGE(p2) at 0x1000 -> page 2");
    assert_eq!(words[0x1800], 0x3000 | 0x18, "PAGE(p3) at 0x1800 -> page 3");
}

#[test]
#[should_panic(expected = "asm: PAGE(nonexistent) label not found")]
fn panics_when_page_label_missing() {
    let src = "    org 0x0000\n    movlw PAGE(nonexistent)\n    end\n";
    let _ = assemble(src);
}

#[test]
fn low_high_page_accept_numeric_operands() {
    // `LOW(<n>)`/`HIGH(<n>)`/`PAGE(<n>)` with a NUMBER operand — hex
    // (padded or not: `LOW(0x2A)` and `LOW(0x4)`), or decimal — resolve as
    // plain literals (LOW = n & 0xFF, HIGH = (n >> 8) & 0xFF, PAGE =
    // (n >> 11) << 3), the same semantics as the label form. gpasm accepts
    // `LOW(0x2A)`/`HIGH(0x123)`/`LOW(35)` (verified), so a numeric operand
    // is valid assembler input; the pre-issue-16 code panicked
    // `LOW(0x2A) label not found` instead of resolving.
    let src = "    org 0x0000\n\
        movlw LOW(0x2A)\n\
        movlw HIGH(0x123)\n\
        movlw PAGE(0x1234)\n\
        movlw LOW(0x4)\n\
        movlw LOW(35)\n\
        movlw LOW(0x2a)\n\
        end\n";
    let words = assemble(src);
    assert_eq!(words[0], 0x3000 | 0x2A, "LOW(0x2A)");
    assert_eq!(words[1], 0x3000 | 0x01, "HIGH(0x123)");
    assert_eq!(
        words[2],
        0x3000 | 0x10,
        "PAGE(0x1234) = (0x1234 >> 11) << 3"
    );
    assert_eq!(words[3], 0x3000 | 0x04, "LOW(0x4) — unpadded hex operand");
    assert_eq!(words[4], 0x3000 | 0x23, "LOW(35) — decimal operand");
    assert_eq!(words[5], 0x3000 | 0x2A, "LOW(0x2a) — lowercase hex operand");
}

// ---- Milestone 11 final wave: .org monotonicity ----

#[test]
#[should_panic(expected = "backward .org")]
fn panics_on_backward_org() {
    // An `.org` can only pad FORWARD: the isel page pads and the pinned
    // table-section start always sit at or ahead of the running address. A
    // backward `.org` (here 0x0000 from a running address of 0x0003) would
    // overwrite already-emitted words, silently relocating code — e.g. a
    // later pass (banking) inserting words could push a page base
    // backwards, and the silent overwrite would misbranch. It must panic
    // loudly instead.
    let src =
        "    org 0x0000\n    movlw 0x01\n    movlw 0x02\n    movlw 0x03\n    org 0x0000\n    end\n";
    let _ = assemble(src);
}

#[test]
fn forward_org_and_same_address_org_assemble() {
    // Forward pads (the M11 `.org 0x0800` page pads) and a no-op `.org` at
    // the CURRENT address (an exact-boundary continuation anchor — the same
    // `.org` the overflow pad already emitted) are both monotone and must
    // assemble; only a strictly backward target panics.
    let src = "    org 0x0000\n    movlw 0x01\n    org 0x0800\n    movlw 0x02\n    org 0x0801\n    movlw 0x03\n    end\n";
    let words = assemble(src);
    assert_eq!(words[0x000], 0x3001);
    assert_eq!(words[0x800], 0x3002);
    assert_eq!(
        words[0x801], 0x3003,
        "same-address .org emits the next word in place"
    );
}

// ---- Milestone 10: const-table window-fit directives ----

#[test]
#[should_panic(expected = "crosses its 256-byte window")]
fn panics_when_small_table_crosses_its_window() {
    // M10 fix: a table whose base + size leaves its 256-byte window would
    // silently misread through the computed `ADDLW LOW(base); MOVWF PCL`
    // jump (reads past 0xFF wrap into the next window with the wrong
    // PCLATH). `.table t 200` at base 0x40: LOW 0x40 + 200 = 0x108 > 0x100 —
    // the assembler must reject it loudly, not assemble a misread.
    let src = "    org 0x0040\n    .table t 200\nt:\n    retlw 0x00\n    end\n";
    let _ = assemble(src);
}

#[test]
#[should_panic(expected = "must be 256-aligned")]
fn panics_when_chunked_table_base_is_misaligned() {
    // M10 fix: a chunked (> 255 byte) table's chunk-0 base must be
    // 256-aligned so chunk 1 (immediately after chunk 0) also has LOW == 0;
    // a misaligned chunk base silently wraps reads past the window end.
    let src = "    org 0x0040\n    .table t 300\nt:\n    retlw 0x00\n    end\n";
    let _ = assemble(src);
}

#[test]
fn small_table_in_nonzero_window_assembles_and_resolves() {
    // A single-entry table that FITS its window is legal: `.table t 40` at
    // base 0x40 (LOW 0x40 + 40 = 0x68 <= 0x100) assembles, and the labels
    // resolve for the reader's PCLATH set and computed jump.
    let mut src = String::from("    org 0x0040\n    .table t 40\nt:\n");
    for i in 0..40 {
        src.push_str(&format!("    retlw 0x{i:02X}\n"));
    }
    src.push_str("    movlw LOW(t)\n    movlw HIGH(t)\n    end\n");
    let words = assemble(&src);
    assert_eq!(words[0x40 + 40], 0x3000 | 0x40, "LOW(t) = 0x40");
    assert_eq!(words[0x40 + 41], 0x3000 | 0x00, "HIGH(t) = 0 (window 0)");
}

#[test]
fn align_and_table_keep_chunked_labels_at_low_zero() {
    // `.align 256` pads with NOP words to the next 256-word boundary, so a
    // chunked table's base and its immediately-following chunk-1 label both
    // sit at LOW == 0 — the window-fit rule for a > 255-byte table. The
    // `movlw LOW(t)` / `movlw LOW(t_1)` / `movlw HIGH(t_1)` operands
    // resolve through the symbol table.
    let mut src = String::from("    org 0x002A\n    nop\n    .align 256\n    .table t 300\nt:\n");
    for i in 0..256 {
        src.push_str(&format!("    retlw 0x{i:02X}\n"));
    }
    src.push_str("t_1:\n");
    for i in 256..300 {
        src.push_str(&format!("    retlw 0x{i:02X}\n"));
    }
    src.push_str("    movlw LOW(t)\n    movlw LOW(t_1)\n    movlw HIGH(t_1)\n    end\n");
    let words = assemble(&src);
    // org 0x2A + nop -> 0x2B, `.align 256` pads to 0x100: t at 0x100, t_1
    // immediately after chunk 0 at 0x200, then the three movlw probes.
    let probe = 0x100 + 256 + 44;
    assert_eq!(words[probe], 0x3000, "LOW(t) = 0 (aligned base)");
    assert_eq!(
        words[probe + 1],
        0x3000,
        "LOW(t_1) = 0 (chunk 1 right after chunk 0)"
    );
    assert_eq!(words[probe + 2], 0x3002, "HIGH(t_1) = 2 (t_1 at 0x200)");
}
