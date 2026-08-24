/// Assign GPR banks to file-register operands.
///
/// Milestone 4: the isel stage emits physical (fully-paged) addresses. This
/// pass scans the assembly, infers each file-register operand's memory bank
/// from its address, inserts a `BANKSEL` when the tracked current bank
/// differs, and rewrites the operand to the 7-bit bank-relative address
/// (`physical & 0x7F`). The core registers mirrored into every bank and the
/// common GPR block (`0x70..=0x7F`) need no banking; a non-mirrored bank-0
/// SFR (PORTA 0x05) is banked like bank-0 GPR and a high-bank SFR (the
/// 887's `0x188` ANSEL) is banked like a GPR. Literal-immediate ops are
/// skipped.
///
/// The tracked bank is reset to UNKNOWN at every label (a branch target — the
/// runtime bank can arrive there from any arm, so the linear predecessor's
/// bank is not reliable); the next banked operand after a reset emits a FULL
/// `BANKSEL` that re-establishes both RP bits. Between labels the tracking
/// still removes redundant switches on straight-line code. A CALL to a
/// callee whose exit bank is provable (issue #13) keeps tracking that bank
/// instead of resetting.
///
/// `BANKSEL <n>` selects bank `n` by setting/clearing the two RP bits of
/// `STATUS` (RP0 = bit 5, RP1 = bit 6); only the bits that change are
/// emitted (`BCF`/`BSF STATUS, 5/6` — numeric bit operands, so no
/// `RP0`/`RP1` symbol definitions are needed anywhere). The bank-select
/// forms recognized are `BCF/BSF STATUS, 5/6` (comma attached to either
/// token), the same by STATUS's register address (`0x03`), and `MOVWF
/// STATUS` (by name or address), which writes all of STATUS from W and
/// makes the tracked bank unknowable.
///
/// # Panics
///
/// `Device::bank_of` panics if any file-register operand is a
/// non-canonical alias of common RAM (`0xF0` is `0x70` seen from bank 1),
/// which no stage may emit.
use std::collections::{HashMap, HashSet};

use device::Device;

const LITERAL_OPS: [&str; 7] = [
    "MOVLW", "ADDLW", "ANDLW", "IORLW", "XORLW", "SUBLW", "RETLW",
];

/// The skip-conditional ops: the next instruction runs only when the tested
/// bit/byte is clear/set/zero/nonzero. A banked operand under one of these
/// is CONDITIONAL — the exit-bank analysis must join both paths (the
/// operand may or may not run, so its bank may or may not be selected).
const SKIP_OPS: [&str; 4] = ["BTFSC", "BTFSS", "INCFSZ", "DECFSZ"];

/// The STATUS register's address (the PIC16F877A's register 3).
const STATUS_ADDR: u16 = 0x03;

/// A bank-select op's effect on the RP bits, or `None` when the line is not
/// a bank-select op. Recognized forms (issue #13 item 3):
/// - `BCF/BSF STATUS, 5/6` — the comma attached to either token;
/// - `BCF/BSF 0x03, 5/6` — STATUS by its register address;
/// - `MOVWF STATUS` / `MOVWF 0x03` — writes all of STATUS from W: the RP
///   bits become UNKNOWABLE (the `Some(None)` case).
/// The bit operand is matched by its numeric value (5 = RP0, 6 = RP1), so
/// `RP0`/`RP1` symbol forms would need the equ table — the isel output
/// always uses the numeric forms, and hand-written asm in the fixtures does
/// too.
fn bank_op_effect(mne: &str, toks: &[&str]) -> Option<Option<u8>> {
    // The comma may be attached to the register token (`STATUS,5`) or
    // separate (`STATUS, 5`); both are the same instruction.
    let (reg, bit) = match toks.get(2) {
        Some(b) => (
            toks.get(1)
                .copied()
                .unwrap_or("")
                .trim_end_matches([',', ';', ')']),
            b.trim_end_matches([',', ';', ')']),
        ),
        None => {
            // `BCF STATUS,5` — the comma is inside the single operand token.
            let t = toks.get(1).copied().unwrap_or("");
            match t.find(',') {
                Some(i) => (t[..i].trim(), t[i + 1..].trim_end_matches([',', ';', ')'])),
                None => (t.trim_end_matches([',', ';', ')']), ""),
            }
        }
    };
    let is_status = reg == "STATUS"
        || reg
            .strip_prefix("0x")
            .and_then(|h| u16::from_str_radix(h, 16).ok())
            == Some(STATUS_ADDR);
    if mne == "MOVWF" && is_status {
        return Some(None); // all of STATUS written from W: bank unknowable
    }
    if (mne == "BCF" || mne == "BSF") && is_status {
        let on = mne == "BSF";
        match bit {
            "5" => return Some(Some(if on { 1 } else { 0 })),
            "6" => return Some(Some(if on { 2 } else { 0 })),
            _ => return None, // a STATUS bit that is not an RP bit
        }
    }
    None
}

/// The bank a file-register operand selects, or `None` when the operand is
/// bank-independent (a mirrored SFR, common RAM, or a literal). The operand
/// token is the first after the mnemonic, with any trailing
/// comma/semicolon stripped.
fn operand_bank(device: &Device, mne: &str, toks: &[&str]) -> Option<u8> {
    if LITERAL_OPS.contains(&mne) {
        return None;
    }
    let op = toks.get(1).copied()?;
    let hex = op.trim_end_matches([',', ';', ')']).strip_prefix("0x")?;
    let v = u16::from_str_radix(hex, 16).ok()?;
    device.bank_of(v)
}

// ---------------------------------------------------------------------------
// Issue #13 item 2: the CALL-exit-bank analysis
// ---------------------------------------------------------------------------

/// A set of possible banks (bit `i` set = bank `i` possible). The join of
/// two sets is the bitwise OR; `0x0F` (all four banks) is the UNKNOWN set.
/// A function's exit bank is provable iff the joined set of every path's
/// exit is a single bank.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BankSet(u8);

impl BankSet {
    const UNKNOWN: BankSet = BankSet(0x0F);
    fn single(b: u8) -> BankSet {
        BankSet(1 << b)
    }
    fn join(self, o: BankSet) -> BankSet {
        BankSet(self.0 | o.0)
    }
    fn is_single(self) -> bool {
        self.0.count_ones() == 1
    }
    fn single_bank(self) -> u8 {
        self.0.trailing_zeros() as u8
    }
}

/// Split the text into per-function regions: every `CALL <name>` target's
/// body, from its label to the next CALL-target label (or the end of the
/// text). Internal labels (`tmpN`, `{func}_L{label}`) are never CALL
/// targets, so they stay inside their function's region; the ISR and
/// `__start` are never CALLed, so they have no region (their exit banks
/// are never needed). Returns the CALL-target set and the regions.
fn function_regions(asm: &str) -> (HashSet<String>, HashMap<String, Vec<&str>>) {
    let mut call_targets: HashSet<String> = HashSet::new();
    for line in asm.lines() {
        let toks: Vec<&str> = line.trim_start().split_whitespace().collect();
        if toks.first() == Some(&"CALL") {
            if let Some(t) = toks.get(1) {
                call_targets.insert(t.trim_end_matches([',', ';', ')']).to_string());
            }
        }
    }
    let lines: Vec<&str> = asm.lines().collect();
    let mut regions: HashMap<String, Vec<&str>> = HashMap::new();
    let mut cur: Option<(String, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if let Some(name) = t.strip_suffix(':') {
            if call_targets.contains(name) {
                if let Some((f, s)) = cur.take() {
                    regions.insert(f, lines[s..i].to_vec());
                }
                cur = Some((name.to_string(), i));
            }
        }
    }
    if let Some((f, s)) = cur.take() {
        regions.insert(f, lines[s..].to_vec());
    }
    (call_targets, regions)
}

/// The provable exit bank of `func` entered with `entry`, or UNKNOWN when
/// not provable. Memoized on `(func, entry)`; the call graph is a DAG
/// (recursion is rejected by `callgraph::check_depth`), so the recursion
/// through `walk_region` terminates.
fn func_exit_bank(
    device: &Device,
    func: &str,
    entry: BankSet,
    call_targets: &HashSet<String>,
    regions: &HashMap<String, Vec<&str>>,
    memo: &mut HashMap<(String, BankSet), BankSet>,
) -> BankSet {
    if let Some(&v) = memo.get(&(func.to_string(), entry)) {
        return v;
    }
    let Some(region) = regions.get(func) else {
        // No region (not a CALL target / not in the text): conservative.
        return BankSet::UNKNOWN;
    };
    let v = walk_region(device, region, entry, call_targets, regions, memo);
    memo.insert((func.to_string(), entry), v);
    v
}

/// Walk one function region from its entry with the symbolic bank set
/// `entry`, joining every path's exit. The walk mirrors the pass's own
/// semantics: a banked operand pins the bank, a bank op applies its effect
/// (MOVWF STATUS makes it unknowable), a CALL joins the callee's exit bank,
/// a skip op (BTFSC/BTFSS/INCFSZ/DECFSZ) forks into both paths, a GOTO
/// jumps, and RETURN/RETLW/RETFIE record an exit. A path that falls off the
/// region's end without returning is not a provable exit (UNKNOWN).
fn walk_region(
    device: &Device,
    region: &[&str],
    entry: BankSet,
    call_targets: &HashSet<String>,
    regions: &HashMap<String, Vec<&str>>,
    memo: &mut HashMap<(String, BankSet), BankSet>,
) -> BankSet {
    let mut label_idx: HashMap<String, usize> = HashMap::new();
    for (i, line) in region.iter().enumerate() {
        if let Some(name) = line.trim_start().strip_suffix(':') {
            label_idx.insert(name.to_string(), i);
        }
    }
    // Precompute asm-block membership: verbatim lines between markers are
    // opaque and clobber the bank to UNKNOWN. The markers themselves also
    // reset to UNKNOWN.
    let mut asm_inside = vec![false; region.len()];
    {
        let mut in_asm = false;
        for (i, line) in region.iter().enumerate() {
            let t = line.trim_start();
            if t.starts_with("; --- asm start ---") {
                in_asm = true;
                // marker itself not considered inside, but handled as barrier
                continue;
            }
            if t.starts_with("; --- asm end ---") {
                in_asm = false;
                continue;
            }
            asm_inside[i] = in_asm;
        }
    }
    let mut exits = BankSet(0);
    let mut work: Vec<(usize, BankSet)> = vec![(0, entry)];
    let mut visited: HashSet<(usize, BankSet)> = HashSet::new();
    while let Some((i, banks)) = work.pop() {
        if !visited.insert((i, banks)) {
            continue;
        }
        let Some(line) = region.get(i) else {
            // Fell off the end without returning: not a provable exit.
            exits = exits.join(BankSet::UNKNOWN);
            continue;
        };
        let trimmed = line.trim_start();
        if trimmed.starts_with("; --- asm start ---") || trimmed.starts_with("; --- asm end ---") {
            work.push((i + 1, BankSet::UNKNOWN));
            continue;
        }
        if asm_inside[i] {
            // Inside verbatim Asm: opaque, bank clobbered.
            work.push((i + 1, BankSet::UNKNOWN));
            continue;
        }
        let toks: Vec<&str> = line.trim_start().split_whitespace().collect();
        let Some(mne) = toks.first().copied() else {
            work.push((i + 1, banks));
            continue;
        };
        if mne.ends_with(':') {
            work.push((i + 1, banks));
            continue;
        }
        if mne == "org" || mne == "end" || mne.starts_with('.') {
            work.push((i + 1, banks));
            continue;
        }
        if mne == "RETURN" || mne == "RETLW" || mne == "RETFIE" {
            exits = exits.join(banks);
            continue;
        }
        if mne == "GOTO" {
            if let Some(target) = toks.get(1) {
                let t = target.trim_end_matches([',', ';', ')']);
                if let Some(&ti) = label_idx.get(t) {
                    work.push((ti, banks));
                } else {
                    exits = exits.join(BankSet::UNKNOWN);
                }
            }
            continue;
        }
        if mne == "CALL" {
            if let Some(target) = toks.get(1) {
                let callee = target.trim_end_matches([',', ';', ')']);
                let eb = func_exit_bank(device, callee, banks, call_targets, regions, memo);
                work.push((i + 1, eb));
            }
            continue;
        }
        if SKIP_OPS.contains(&mne) {
            // The next instruction is conditional: fork into the skip-taken
            // path (the instruction does not run) and the not-taken path
            // (it runs, applying its own effect).
            let mut j = i + 1;
            while j < region.len() && region[j].trim_start().ends_with(':') {
                j += 1;
            }
            work.push((j + 1, banks));
            work.push((j, banks));
            continue;
        }
        if let Some(effect) = bank_op_effect(mne, &toks) {
            let nb = match effect {
                Some(b) => BankSet::single(b),
                None => BankSet::UNKNOWN,
            };
            work.push((i + 1, nb));
            continue;
        }
        if let Some(b) = operand_bank(device, mne, &toks) {
            work.push((i + 1, BankSet::single(b)));
            continue;
        }
        work.push((i + 1, banks));
    }
    exits
}

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
fn is_bank0_only(device: &Device, asm: &str) -> bool {
    // Any Asm block clobbers the bank: the isel bracket `; --- asm start ---`
    // marks opaque verbatim that may touch STATUS/RP bits arbitrarily, so the
    // program cannot be proved to stay in bank 0.
    if asm.contains("; --- asm start ---") || asm.contains("; --- asm end ---") {
        return false;
    }
    for line in asm.lines() {
        let toks: Vec<&str> = line.trim_start().split_whitespace().collect();
        let Some(mne) = toks.first().copied() else {
            continue;
        };
        // Any bank-select op (BCF/BSF STATUS by name or address, MOVWF
        // STATUS) touches the bank bits: the pass cannot prove the bank
        // stays 0 at every label/CALL.
        if bank_op_effect(mne, &toks).is_some() {
            return false;
        }
        // Directives (`org`, `.align`, `.table`, `end`, ...) take
        // addresses/literals, never file-register operands: an `.org 0x0800`
        // page pad must not be range-checked.
        if mne == "org" || mne == "end" || mne.starts_with('.') {
            continue;
        }
        if let Some(bank) = operand_bank(device, mne, &toks) {
            // Bank 0 is fine; any NONZERO bank disqualifies.
            if bank != 0 {
                return false;
            }
        }
    }
    true
}

/// Insert `BANKSEL` before file-register operands whose bank differs from the
/// tracked current bank — or whenever the tracked bank is unknown (just after
/// a label) — and rewrite banked operands to `physical & 0x7F`.
pub fn assign_banks(device: &Device, asm: &str) -> String {
    // Issue #16 (left over from #13): a bank-0-only program provably never
    // leaves bank 0, so the label/CALL resets below can be skipped entirely
    // instead of emitting the dead full BANKSEL preamble after every label
    // and CALL. The scan is conservative — any banked operand, hand-written
    // bank select (any form), or `MOVWF STATUS` disables the skip.
    let bank0_only = is_bank0_only(device, asm);
    // Issue #13 item 2: a CALL to a callee whose exit bank is provable keeps
    // tracking that bank instead of resetting to unknown — the full BANKSEL
    // after the CALL is redundant when the caller's next operand is in the
    // callee's exit bank. The analysis walks each callee's region with the
    // pass's own semantics (banked operands pin the bank, bank ops apply
    // their effect, skips fork, GOTOs jump, CALLs join the callee's exit)
    // and joins every path's exit; a single-bank join is provable.
    let (call_targets, regions) = function_regions(asm);
    let mut exit_memo: HashMap<(String, BankSet), BankSet> = HashMap::new();
    let mut out = String::new();
    let mut known = true; // false = the tracked bank is unknown (entered at a branch target)
    let mut rp0 = false; // STATUS, bit 5
    let mut rp1 = false; // STATUS, bit 6
    let mut in_asm = false;
    for line in asm.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("; --- asm start ---") {
            in_asm = true;
            known = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.starts_with("; --- asm end ---") {
            in_asm = false;
            known = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_asm {
            out.push_str(line);
            out.push('\n');
            continue;
        }
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
        // Issue #13 item 2: when the callee's exit bank is PROVABLE (a
        // single-bank join of every path), the tracked bank becomes that
        // bank — the full BANKSEL after the CALL is redundant when the
        // caller's next operand is in it.
        if mne == "CALL" {
            if bank0_only {
                known = true;
            } else if let Some(target) = toks.get(1) {
                let callee = target.trim_end_matches([',', ';', ')']);
                let cur = BankSet::single(u8::from(rp0) | (u8::from(rp1) << 1));
                let eb =
                    func_exit_bank(device, callee, cur, &call_targets, &regions, &mut exit_memo);
                if eb.is_single() {
                    known = true;
                    rp0 = eb.single_bank() & 1 == 1;
                    rp1 = eb.single_bank() & 2 == 2;
                } else {
                    known = false;
                }
            } else {
                known = false;
            }
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

        // A bank-select op (BCF/BSF STATUS by name or address, MOVWF STATUS)
        // updates the tracked bank. MOVWF STATUS writes all of STATUS from W:
        // the RP bits become unknowable, so the next banked operand gets a
        // FULL BANKSEL.
        if let Some(effect) = bank_op_effect(mne, &toks) {
            match effect {
                Some(b) => {
                    rp0 = b & 1 == 1;
                    rp1 = b & 2 == 2;
                    known = true;
                }
                None => known = false,
            }
            out.push_str(line);
            out.push('\n');
            continue;
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
                if let Some(bank) = device.bank_of(v) {
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
