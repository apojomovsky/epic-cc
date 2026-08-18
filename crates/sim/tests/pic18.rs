use pic14_sim::{parse_hex_pic18, Pic18};

#[test]
fn a_single_nop_advances_pc_by_two_bytes_and_halts() {
    let mut p = Pic18::new(vec![0x0000]); // one NOP word
    p.run(10);
    assert_eq!(p.pc(), 2); // PC is a BYTE address on PIC18
    assert!(p.halted());
}

#[test]
fn ram_is_directly_readable_and_writable() {
    let mut p = Pic18::new(vec![0x0000]);
    p.ram_mut()[0x55] = 0x42;
    assert_eq!(p.ram()[0x55], 0x42);
}

#[test]
fn parse_hex_pic18_decodes_intel_hex_into_words() {
    let hex = asm::to_hex(&[0x55AA]);
    let words = parse_hex_pic18(&hex);
    assert_eq!(words[0], 0x55AA);
}
