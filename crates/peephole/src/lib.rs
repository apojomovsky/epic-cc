/// Peephole-optimize PIC-8 assembly.
///
/// Milestone 11: the tracked-literal PCLATH elision. The M11 isel always
/// emits a `MOVLW PAGE(<target>); MOVWF PCLATH` set before every CALL and a
/// `MOVLW PAGE(<cur_func>); MOVWF PCLATH` restore right after. Nothing else
/// writes PCLATH: CALL/GOTO/RETURN leave it unchanged, and `MOVWF PCL`
/// (a reader's computed goto) only reads it. So a new
/// `MOVLW <k>; MOVWF PCLATH` pair is redundant whenever `k` equals the last
/// PCLATH literal written — dropping it cannot change the behavior.
///
/// The tracked literal persists across CALL/GOTO/labels/directives; only
/// `MOVWF PCLATH` updates it. Operands are compared canonically: numeric
/// literals are normalized to `0xXX` hex (so `0x08 == 0x08`), symbolic
/// operands (`PAGE(main)`, `HIGH(table)`) are compared as strings — an
/// identical token resolves to an identical literal, so eliding it is sound;
/// differing tokens are conservatively kept. A standalone `MOVWF PCLATH`
/// (writing the unknown value currently in W) forgets the tracked literal.
pub fn optimize(asm: &str) -> String {
    let lines: Vec<&str> = asm.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    // Canonical literal of the last `MOVLW k; MOVWF PCLATH` pair.
    let mut tracked: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_movlw(line) && i + 1 < lines.len() && is_movwf_pclath(lines[i + 1]) {
            let literal = canonical_literal(movlw_operand(line));
            if tracked.as_deref() == Some(literal.as_str()) {
                // Same literal already in PCLATH: the new pair is redundant.
                i += 2;
                continue;
            }
            out.push(line);
            out.push(lines[i + 1]);
            tracked = Some(literal);
            i += 2;
            continue;
        }
        if is_movwf_pclath(line) {
            // Standalone write of an unknown value: keep it, forget the
            // tracked literal (it no longer reflects PCLATH).
            out.push(line);
            tracked = None;
            i += 1;
            continue;
        }
        out.push(line);
        i += 1;
    }
    let mut result = out.join("\n");
    if asm.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn is_movlw(line: &str) -> bool {
    line.trim_start().starts_with("MOVLW ")
}

fn is_movwf_pclath(line: &str) -> bool {
    line.trim_start().starts_with("MOVWF PCLATH")
}

fn movlw_operand(line: &str) -> &str {
    line.trim_start().strip_prefix("MOVLW").unwrap_or(line).trim()
}

/// Normalize an operand for equality: numeric literals to `0xXX` hex, so
/// `0x08` and `0x8` compare equal; symbolic operands (`PAGE(...)`,
/// `HIGH(...)`) are kept verbatim as tokens.
fn canonical_literal(operand: &str) -> String {
    let t = operand.trim();
    if let Some(v) = parse_literal(t) {
        format!("0x{v:02X}")
    } else {
        t.to_string()
    }
}

fn parse_literal(s: &str) -> Option<u16> {
    if let Some(hex) = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u16>().ok()
    }
}
