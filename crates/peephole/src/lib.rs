/// Peephole-optimizes PIC-8 assembly: elides a redundant tracked-literal PCLATH set.
///
/// isel emits a `MOVLW PAGE(<target>); MOVWF PCLATH` set before every CALL and a
/// `MOVLW PAGE(<cur_func>); MOVWF PCLATH` restore after, except for same-page calls
/// which isel skips itself. Nothing else writes PCLATH: CALL, GOTO, and RETURN leave
/// it unchanged, and `MOVWF PCL` only reads it. A new `MOVLW <k>; MOVWF PCLATH` pair
/// with `k` equal to the last written literal changes nothing, so the pass drops it.
///
/// The pass acts as a defensive standalone step for the driver path: isel already
/// omits same-page restores, so this collapses any residual redundant pair and keeps
/// any pair it cannot prove redundant. Sound for any input.
///
/// The tracked literal resets at every LABEL: a branch target sees the PCLATH of the
/// path taken, not the text order. This reset keeps multi-page programs sound: a CALL
/// set stays even when a previous function restore wrote the same literal earlier in
/// the text. Operands compare canonically: numeric literals normalize to `0xXX` hex
/// and symbolic operands (`PAGE(main)`, `HIGH(table)`) compare as tokens, where an
/// identical token resolves to an identical literal. A standalone `MOVWF PCLATH`
/// writes the unknown value in W and clears the tracked literal.
use ir::SrcLoc;

pub fn optimize(asm: &str) -> String {
    optimize_with_locs(asm, &[]).0
}

/// `optimize` plus a parallel per-line source-location vector, kept
/// index-aligned with the returned text. An elided `MOVLW k; MOVWF PCLATH`
/// pair drops its two locs with it; every other line keeps its own loc.
/// `locs` may be empty (the text-only path); when non-empty it must be the
/// same length as `asm`'s lines.
pub fn optimize_with_locs(asm: &str, locs: &[Option<SrcLoc>]) -> (String, Vec<Option<SrcLoc>>) {
    let lines: Vec<&str> = asm.lines().collect();
    let locs = locs.to_vec();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut out_locs: Vec<Option<SrcLoc>> = Vec::with_capacity(lines.len());
    // Canonical literal of the last `MOVLW k; MOVWF PCLATH` pair.
    let mut tracked: Option<String> = None;
    let mut in_asm = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let cur_loc = locs.get(i).cloned().flatten();
        let trimmed = line.trim_start();
        if trimmed.starts_with("; --- asm start ---") {
            in_asm = true;
            out.push(line);
            out_locs.push(cur_loc);
            tracked = None;
            i += 1;
            continue;
        }
        if trimmed.starts_with("; --- asm end ---") {
            in_asm = false;
            out.push(line);
            out_locs.push(cur_loc);
            tracked = None;
            i += 1;
            continue;
        }
        if in_asm {
            out.push(line);
            out_locs.push(cur_loc);
            i += 1;
            continue;
        }
        // A label is a branch target: the runtime PCLATH there follows the path
        // taken, not the linear text order. The tracked literal predicts PCLATH
        // on straight-line code only, so it clears at every label. Clearing at
        // function-boundary labels keeps each CALL set intact across contexts.
        if is_label(line) {
            out.push(line);
            out_locs.push(cur_loc);
            tracked = None;
            i += 1;
            continue;
        }
        if is_movlw(line) && i + 1 < lines.len() && is_movwf_pclath(lines[i + 1]) {
            // The asm guard above keeps pairs out of inline assembly; this also
            // refuses a pair that would cross into an asm marker.
            let next_trimmed = lines[i + 1].trim_start();
            if next_trimmed.starts_with("; --- asm") {
                out.push(line);
                out_locs.push(cur_loc);
                i += 1;
                continue;
            }
            let literal = canonical_literal(movlw_operand(line));
            if tracked.as_deref() == Some(literal.as_str()) {
                // Same literal already in PCLATH: the new pair is redundant.
                i += 2;
                continue;
            }
            out.push(line);
            out_locs.push(cur_loc);
            out.push(lines[i + 1]);
            out_locs.push(locs.get(i + 1).cloned().flatten());
            tracked = Some(literal);
            i += 2;
            continue;
        }
        if is_movwf_pclath(line) {
            // A standalone write carries an unknown value: the pass keeps it and
            // clears the tracked literal, which stops reflecting PCLATH.
            out.push(line);
            out_locs.push(cur_loc);
            tracked = None;
            i += 1;
            continue;
        }
        out.push(line);
        out_locs.push(cur_loc);
        i += 1;
    }
    let mut result = out.join("\n");
    if asm.ends_with('\n') {
        result.push('\n');
    }
    (result, out_locs)
}

fn is_movlw(line: &str) -> bool {
    line.trim_start().starts_with("MOVLW ")
}

fn is_movwf_pclath(line: &str) -> bool {
    line.trim_start().starts_with("MOVWF PCLATH")
}

fn is_label(line: &str) -> bool {
    let t = line.trim_start();
    t.ends_with(':') && !t.starts_with('.')
}

fn movlw_operand(line: &str) -> &str {
    line.trim_start()
        .strip_prefix("MOVLW")
        .unwrap_or(line)
        .trim()
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
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u16>().ok()
    }
}
