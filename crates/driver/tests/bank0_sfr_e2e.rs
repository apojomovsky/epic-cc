//! Issue #112 acceptance: a non-mirrored bank-0 SFR access after a banked
//! operand must select bank 0 first, or the write lands on the bank-1 SFR
//! at the same offset (TRISA 0x85 instead of PORTA 0x05). The in-repo
//! simulator models 0x01-0x1F as bank-independent (the same misconception
//! the compiler had), so the behavioral gate is the emitted assembly: the
//! bank-0 SFR store must be preceded by `BCF STATUS, 5` (RP0 = 0).

use std::process::Command;

#[test]
fn bank0_sfr_store_selects_bank0_after_a_banked_operand() {
    let asm_path = std::env::temp_dir().join("bank0_sfr_e2e.asm");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/bank0_sfr.c",
            "-o",
            asm_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "--emit",
            "asm",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).expect("read emitted asm");
    let _ = std::fs::remove_file(&asm_path);
    // The TRISA store (bank 1) then the PORTA store (bank 0): the second
    // must select bank 0 first, or it writes TRISA again.
    let store1 = asm.find("    MOVWF 0x05").expect("TRISA store");
    let sel = asm.rfind("    BCF STATUS, 5").expect("bank-0 select");
    let store2 = asm.rfind("    MOVWF 0x05").expect("PORTA store");
    assert!(
        sel > store1 && sel < store2,
        "the bank-0 select must sit between the two stores:\n{asm}"
    );
}
