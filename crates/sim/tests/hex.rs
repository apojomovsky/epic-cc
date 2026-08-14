use pic14_sim::parse_hex;

#[test]
fn decodes_little_endian_words() {
    // goto 0x005 -> 0x2805 -> bytes 05 28 ; movlw 0xAB -> 0x30AB -> AB 30
    let hex = ":020000040000FA\n:040000000528AB30F4\n:00000001FF\n";
    let words = parse_hex(hex);
    assert_eq!(words[0], 0x2805);
    assert_eq!(words[1], 0x30AB);
    assert_eq!(words[2], 0x0000);
    assert_eq!(words[3], 0x0000);
}
