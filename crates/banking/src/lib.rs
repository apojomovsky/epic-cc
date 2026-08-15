/// Assign GPR banks to file-register operands.
///
/// Milestone 1: bank-0-only. Every file-register operand must be in the
/// `0x00..=0x7F` range (bank 0 / common), so no `BANKSEL` is required.
/// The assembly text is returned unchanged.
///
/// # Panics
///
/// Panics if any file-register `0x...` operand is `>= 0x80` (outside the
/// bank-0/1/2/3 GPR range supported by milestone 1). Literal-immediate
/// operands (`MOVLW`/`ADDLW`/`ANDLW`/`IORLW`/`XORLW`/`SUBLW`/`RETLW`) are
/// 8-bit constants, not addresses, and are not range-checked.
pub fn assign_banks(asm: &str) -> String {
    for line in asm.lines() {
        let mne = line.split_whitespace().next().unwrap_or("");
        if matches!(
            mne,
            "MOVLW" | "ADDLW" | "ANDLW" | "IORLW" | "XORLW" | "SUBLW" | "RETLW"
        ) {
            continue; // operand is a literal immediate, not a file register
        }
        for tok in line.split_whitespace() {
            if let Some(hex) = tok.strip_prefix("0x") {
                // operands carry trailing punctuation (e.g. `0x20,`)
                let hex = hex.trim_end_matches([',', ';', ')']);
                let v = u16::from_str_radix(hex, 16).unwrap();
                if v >= 0x80 {
                    panic!("banking: operand 0x{v:02X} is outside bank 0/1/2/3 GPR range (milestone 1: bank 0 only)");
                }
            }
        }
    }
    asm.to_string()
}
