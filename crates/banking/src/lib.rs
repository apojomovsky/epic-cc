/// Assign GPR banks to file-register operands.
///
/// Milestone 4: the isel stage emits physical (fully-paged) addresses. This
/// pass scans the assembly, infers each file-register operand's memory bank
/// from its address, inserts a `BANKSEL` when the tracked current bank
/// differs, and rewrites the operand to the 7-bit bank-relative address
/// (`physical & 0x7F`). SFRs (`0x00..=0x1F`) and the common GPR block
/// (`0x70..=0x7F`) need no banking. Literal-immediate ops are skipped.
///
/// The tracked bank is reset to UNKNOWN at every label (a branch target — the
/// runtime bank can arrive there from any arm, so the linear predecessor's
/// bank is not reliable); the next banked operand after a reset emits a FULL
/// `BANKSEL` that re-establishes both RP bits. Between labels the tracking
/// still removes redundant switches on straight-line code.
///
/// `BANKSEL <n>` selects bank `n` by setting/clearing the two RP bits of
/// `STATUS` (RP0 = bit 5, RP1 = bit 6); only the bits that change are
/// emitted (`BCF`/`BSF STATUS, 5/6` — numeric bit operands, so no
/// `RP0`/`RP1` symbol definitions are needed anywhere).
///
/// # Panics
///
/// Panics if any file-register operand lies in an SFR/unused range
/// (`0x80..=0x9F`, `0xF0..=0xFF`, `0x170..=0x19F`, ...) that must never be
/// emitted as a GPR address.
fn bank_of(v: u16) -> Option<u8> {
    match v {
        0x00..=0x1F => None, // SFR
        0x20..=0x6F => Some(0),
        0x70..=0x7F => None, // common
        0xA0..=0xEF => Some(1),
        0x120..=0x16F => Some(2),
        0x1A0..=0x1EF => Some(3),
        other => panic!("banking: operand 0x{other:03X} is not a banked GPR address"),
    }
}

const LITERAL_OPS: [&str; 7] = ["MOVLW", "ADDLW", "ANDLW", "IORLW", "XORLW", "SUBLW", "RETLW"];

/// True when the whole text provably runs in bank 0: no banked GPR operand
/// (0xA0-0xEF / 0x120-0x16F / 0x1A0-0x1EF) anywhere, no hand-written
/// `BCF/BSF STATUS, 5/6` bank select, and no `MOVWF STATUS` (which writes
/// the RP bits from W, an unknowable value). Such a program can never leave
/// bank 0: every label/CALL reset below can be skipped, because the tracked
/// bank is provably 0 everywhere — the reset vector (bank 0) and every
/// fall-through (which never changed the bank) agree, and an ISR's
/// `MOVWF STATUS` restore is excluded by the STATUS-write check. `MOVWF
/// STATUS` programs therefore keep the resets and their layouts are
/// unchanged.
fn is_bank0_only(asm: &str) -> bool {
    for line in asm.lines() {
        let toks: Vec<&str> = line.trim_start().split_whitespace().collect();
        let Some(mne) = toks.first().copied() else {
            continue;
        };
        if mne == "MOVWF" && toks.get(1).map(|t| t.trim_end_matches([',', ';', ')'])) == Some("STATUS") {
            return false;
        }
        // Directives (`org`, `.align`, `.table`, `end`, ...) take
        // addresses/literals, never file-register operands: an `.org 0x0800`
        // page pad must not be range-checked.
        if mne == "org" || mne == "end" || mne.starts_with('.') {
            continue;
        }
        if mne == "BCF" || mne == "BSF" {
            let reg = toks.get(1).copied().unwrap_or("").trim_end_matches([',', ';', ')']);
            let bit = toks.get(2).copied().unwrap_or("").trim_end_matches([',', ';', ')']);
            if reg == "STATUS" && (bit == "5" || bit == "6") {
                return false;
            }
        }
        if LITERAL_OPS.contains(&mne) {
            continue;
        }
        if let Some(op) = toks.get(1).copied() {
            if let Some(hex) = op.trim_end_matches([',', ';', ')']).strip_prefix("0x") {
                if let Ok(v) = u16::from_str_radix(hex, 16) {
                    // Bank 0 is fine; any NONZERO bank disqualifies.
                    if bank_of(v).is_some_and(|b| b != 0) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Insert `BANKSEL` before file-register operands whose bank differs from the
/// tracked current bank — or whenever the tracked bank is unknown (just after
/// a label) — and rewrite banked operands to `physical & 0x7F`.
pub fn assign_banks(asm: &str) -> String {
    // Issue #16 (left over from #13): a bank-0-only program provably never
    // leaves bank 0, so the label/CALL resets below can be skipped entirely
    // instead of emitting the dead full BANKSEL preamble after every label
    // and CALL. The scan is conservative — any banked operand, hand-written
    // `BCF/BSF STATUS, 5/6`, or `MOVWF STATUS` disables the skip.
    let bank0_only = is_bank0_only(asm);
    let mut out = String::new();
    let mut known = true; // false = the tracked bank is unknown (entered at a branch target)
    let mut rp0 = false; // STATUS, bit 5
    let mut rp1 = false; // STATUS, bit 6
    for line in asm.lines() {
        // Collect into a Vec so the BANKSEL-recognition branch below can look
        // ahead without consuming tokens: a BCF/BSF on a banked GPR (not
        // STATUS) must still reach the generic operand-processing path with
        // its operand intact.
        let toks: Vec<&str> = line.trim_start().split_whitespace().collect();
        let Some(mne) = toks.first().copied() else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        // A label is a branch target: the runtime bank can arrive there from
        // any arm, so the linearly-tracked bank is not reliable there. Drop
        // it; the next banked operand (when the needed bank is unknown) gets
        // a FULL BANKSEL that re-establishes both RP bits. In a bank-0-only
        // program the bank provably stays 0, so the reset is skipped.
        if mne.ends_with(':') {
            // In a bank-0-only program the bank provably stays 0, so the
            // reset is skipped (the tracked bank remains known).
            known = bank0_only;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // A CALL is a runtime boundary just like a label: the callee's body
        // (its own BANKSELs and banked operands) can leave the RP bits in any
        // state, and its prologue/epilogue are not visible in the caller's
        // text. The tracked bank must not cross a CALL — a caller's next
        // banked operand gets a FULL BANKSEL, so it is correct no matter what
        // the callee left behind. In a bank-0-only program the callee (like
        // every function) provably runs in bank 0, so the reset is skipped.
        if mne == "CALL" {
            // In a bank-0-only program the callee (like every function)
            // provably runs in bank 0, so the reset is skipped (the tracked
            // bank remains known).
            known = bank0_only;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Directives (`org`, `.align`, `.table`, `end`, ...) are not
        // instructions: their numeric arguments are addresses or literals,
        // never file-register operands. An `.org` target in a GPR/SFR range
        // (an M11 page pad like `.org 0x0800`, or a pinned table-section
        // start) must pass through untouched — BANKSEL-rewriting it would
        // relocate the program.
        if mne == "org" || mne == "end" || mne.starts_with('.') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // `BCF/BSF STATUS, 5/6` — whether emitted here or already present —
        // update the tracked bank.
        if mne == "BCF" || mne == "BSF" {
            let reg = toks.get(1).copied().unwrap_or("").trim_end_matches([',', ';', ')']);
            let bit = toks.get(2).copied().unwrap_or("").trim_end_matches([',', ';', ')']);
            if reg == "STATUS" && (bit == "5" || bit == "6") {
                let on = mne == "BSF";
                if bit == "5" {
                    rp0 = on;
                } else {
                    rp1 = on;
                }
                out.push_str(line);
                out.push('\n');
                continue;
            }
        }

        // Literal-immediate ops take an 8-bit constant, not a file register.
        if LITERAL_OPS.contains(&mne) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Byte- and bit-oriented ops: the file-register operand is the first.
        let Some(op) = toks.get(1).copied() else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if let Some(hex) = op.trim_end_matches([',', ';', ')']).strip_prefix("0x") {
            if let Ok(v) = u16::from_str_radix(hex, 16) {
                if let Some(bank) = bank_of(v) {
                    let cur = u8::from(rp0) | (u8::from(rp1) << 1);
                    if !known || bank != cur {
                        emit_banksel(&mut out, &mut rp0, &mut rp1, bank, !known);
                        known = true;
                    }
                    let rewritten = v & 0x7F;
                    if rewritten != v {
                        out.push_str(&line.replacen(
                            &format!("0x{v:02X}"),
                            &format!("0x{rewritten:02X}"),
                            1,
                        ));
                        out.push('\n');
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn emit_banksel(out: &mut String, rp0: &mut bool, rp1: &mut bool, bank: u8, full: bool) {
    for (bit, cur, target) in [(5, rp0, bank & 1 == 1), (6, rp1, bank & 2 == 2)] {
        if full || *cur != target {
            let op = if target { "BSF" } else { "BCF" };
            out.push_str(&format!("    {op} STATUS, {bit}\n"));
            *cur = target;
        }
    }
}
