use asm::{to_hex, to_hex_regions};

#[test]
fn a_single_chunk_at_zero_matches_to_hex_exactly() {
    let words = vec![0x2830u16, 0x0064, 0x0000];
    assert_eq!(to_hex_regions(&[(0, &words)]), to_hex(&words));
}

#[test]
fn two_chunks_crossing_a_64k_boundary_emit_a_second_extended_address_record() {
    // Chunk 1: one word at byte address 0. Chunk 2: one word at byte
    // address 0x300000 (word address 0x180000), PIC18F4550's config
    // region base. The upper 16 bits differ (0x0000 vs 0x0030), so a
    // second :04 record must appear before the second chunk's data.
    let a = vec![0x1234u16];
    let b = vec![0x9BFFu16];
    let hex = to_hex_regions(&[(0, &a), (0x300000, &b)]);
    let lines: Vec<&str> = hex.lines().collect();
    assert_eq!(lines[0], ":020000040000FA"); // upper=0x0000
    assert!(lines.iter().any(|l| *l == ":020000040030CA")); // upper=0x0030
                                                            // The config word's own data record: 2 bytes, address 0x0000 within
                                                            // the 0x0030 window, low byte 0xFF, high byte 0x9B.
    assert!(hex.contains(":02000000FF9B"));
    assert_eq!(lines.last().unwrap(), &":00000001FF");
}

#[test]
fn chunks_at_the_same_upper_16_bits_share_one_extended_address_record() {
    let a = vec![0x1111u16];
    let b = vec![0x2222u16];
    let hex = to_hex_regions(&[(0, &a), (0x10, &b)]);
    let extended_records = hex.lines().filter(|l| l.ends_with("040000FA")).count();
    assert_eq!(extended_records, 1);
}
