//! TEMPORARY probe (epic-cc#819 investigation, deleted before PR).
#[test]
fn dump_daw_probe() {
    let cases: &[(u8, u8)] = &[
        (0xFF, 0x00),
        (0xFF, 0x01),
        (0xFF, 0x02),
        (0xFF, 0x03),
        (0x9F, 0x00),
        (0x9A, 0x00),
        (0x9A, 0x02),
        (0xA0, 0x00),
    ];
    let mut src = String::from("goto start\nstart:\n");
    for (i, (w, s)) in cases.iter().enumerate() {
        src.push_str(&format!("movlw 0x{s:02X}\nmovwf 0xFD8,A\n"));
        src.push_str(&format!("movlw 0x{w:02X}\ndaw\n"));
        src.push_str(&format!("movff 0xFE8,0x{:03X}\n", 0x100 + 2 * i));
        src.push_str(&format!("movff 0xFD8,0x{:03X}\n", 0x101 + 2 * i));
    }
    src.push_str("spin:\ngoto spin\n");
    let words = asm::assemble_pic18(&src);
    std::fs::create_dir_all("scratch").unwrap();
    std::fs::write("scratch/gdaw.hex", asm::to_hex(&words)).unwrap();
    println!("daw probe: {} words", words.len());
}
