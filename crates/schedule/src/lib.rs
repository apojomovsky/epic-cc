//! Reorder small, provably-independent instruction groups to reduce the
//! bank switches `banking::assign_banks` must later insert (ADR-027,
//! epic-cc#210). Runs between isel and banking in the PIC14 pipeline:
//! `isel -> schedule -> banking -> peephole -> page-fit -> asm`.
//!
//! Phase 1 (this crate's current scope, epic-cc#210) is the
//! classification and region-splitting infrastructure a scheduling
//! transform needs, wired into the pipeline as an identity transform: it
//! classifies every line's bank demand, W/flag reads and writes, and
//! skip-op/skip-target status, and splits the text into the same kind of
//! straight-line regions `banking`/`peephole` already reset their own
//! tracked state at, but does not yet move anything. The actual reorder
//! (a follow-up PR) only ever touches the exact hand-verified shape ADR-027
//! commits to: a single differently-banked instruction sandwiched between
//! same-bank neighbors, with none of the hazards this module already
//! detects.
//!
//! Bank and bank-select classification is `banking`'s (`operand_bank`,
//! `bank_op_effect`, `SKIP_OPS`, `LITERAL_OPS`, all `pub` for this reason),
//! reused rather than duplicated so the two passes never silently disagree
//! about which bank an operand needs.

use banking::{operand_bank, SKIP_OPS};
use device::Device;

/// One classified instruction: everything a reordering decision needs to
/// know about it. Unrecognized mnemonics never reach this type; `classify`
/// treats them as an opaque barrier instead (see `Line::Opaque`), the same
/// "unknown is the conservative case, not a guess" default this codebase
/// already uses for panics over silent miscompiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insn<'a> {
    pub line: &'a str,
    pub mnemonic: &'a str,
    /// The bank a file-register operand needs (`banking::operand_bank`);
    /// `None` for a literal-immediate op, a mirrored SFR, or common RAM.
    pub bank: Option<u8>,
    pub reads_w: bool,
    pub writes_w: bool,
    /// Coarse and sound, not precise: true if this instruction reads or
    /// sets ANY flag bit (Z/C/DC), including an arbitrary `BCF/BSF
    /// STATUS,b` whose bit isn't one of the RP bank-select bits. Phase 1
    /// has no per-bit lattice; any flag-touching instruction is simply
    /// never a move candidate and never something a candidate may cross.
    pub reads_flags: bool,
    pub writes_flags: bool,
    /// The file-register address this instruction reads/writes directly
    /// (not through W), when its operand is a `0x..` literal address. A
    /// symbolic operand (`PCLATH`, `STATUS`, `FSR`, ...) is conservatively
    /// `None` here even though it does name a real register: those are
    /// core registers mirrored into every bank, and the only ones isel
    /// emits by name; a genuinely address-bearing hazard is always a
    /// `0x..` literal in this codebase's own asm output.
    pub file_addr: Option<u16>,
    pub reads_file: bool,
    pub writes_file: bool,
    pub is_skip: bool,
    /// True when the immediately preceding classified line is a skip op:
    /// this instruction is the other half of an atomic, unsplittable
    /// pair (issue #6; `crates/banking/tests/banking.rs:79-87` is the
    /// concrete regression this invariant protects). Never move this
    /// instruction, never move anything into or out of this exact slot.
    pub is_skip_target: bool,
}

/// One line of the flat asm text, classified for scheduling purposes.
/// Every variant that isn't `Insn` is a hazard boundary a reorder may
/// never cross (see `regions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line<'a> {
    Label(&'a str),
    /// `CALL <target>`: the callee's effects are not visible in this
    /// text, mirroring `banking`'s own treatment of a CALL as a runtime
    /// boundary the tracked bank must not cross.
    Call(&'a str),
    /// A `GOTO`/`RETURN`/`RETLW`/`RETFIE`: ends straight-line control
    /// flow here, conservatively treated as a region boundary even
    /// though only `GOTO` is a real branch (this codebase's own `banking`
    /// pass already resets at every label a `GOTO` could target, so
    /// nothing is lost by also stopping a region right where one occurs).
    Terminator(&'a str),
    /// The verbatim inline-asm markers themselves, and any line between
    /// them: opaque, arbitrary effects, mirrors `banking`'s and
    /// `peephole`'s own `in_asm` barrier handling.
    AsmBarrier(&'a str),
    InAsm(&'a str),
    /// `org`/`end`/anything starting with `.` (`.align`, `.table`, ...):
    /// not an instruction, never a hazard source itself, but its exact
    /// position is load-bearing (an `.org` anchor, a `.table`'s alignment
    /// padding), so it is still a barrier: nothing may move across it.
    Directive(&'a str),
    Blank,
    /// An unrecognized mnemonic. Never a move candidate, and (like a
    /// directive) never something a candidate may move across either:
    /// the safe default when this module's classification table doesn't
    /// know an instruction's W/flag/file-register effects.
    Opaque(&'a str),
    Insn(Insn<'a>),
}

impl<'a> Line<'a> {
    /// The original source line, verbatim, for lossless reconstruction.
    pub fn raw(&self) -> &'a str {
        match self {
            Line::Label(s)
            | Line::Call(s)
            | Line::Terminator(s)
            | Line::AsmBarrier(s)
            | Line::InAsm(s)
            | Line::Directive(s)
            | Line::Opaque(s) => s,
            Line::Blank => "",
            Line::Insn(i) => i.line,
        }
    }

    /// True for anything that must never be moved, and must never have
    /// another instruction moved across it: every non-`Insn` variant.
    pub fn is_barrier(&self) -> bool {
        !matches!(self, Line::Insn(_))
    }
}

const TERMINATORS: [&str; 4] = ["GOTO", "RETURN", "RETLW", "RETFIE"];

/// Byte-oriented ops whose second token selects the destination (`W` or
/// `F`); everything else in `read_only` (no destination bit at all: the
/// result always goes back to `f`, or, for `MOVWF`, is a pure write).
const DEST_SELECTABLE: [&str; 12] = [
    "ADDWF", "ANDWF", "COMF", "DECF", "INCF", "IORWF", "MOVF", "RLF", "RRF", "SUBWF", "SWAPF",
    "XORWF",
];
/// Of `DEST_SELECTABLE`, which also read W (the two-operand ALU ops); the
/// rest (`COMF`/`DECF`/`INCF`/`MOVF`/`RLF`/`RRF`/`SWAPF`) read only `f`.
const DEST_SELECTABLE_READS_W: [&str; 5] = ["ADDWF", "ANDWF", "IORWF", "SUBWF", "XORWF"];
/// Of `DEST_SELECTABLE`, which set Z/C/DC: everything except `SWAPF`
/// (sets nothing). `RLF`/`RRF` set C only and `MOVF` sets Z only, both
/// folded into the same conservative `writes_flags` bit.
const DEST_SELECTABLE_SETS_FLAGS: [&str; 11] = [
    "ADDWF", "ANDWF", "COMF", "DECF", "INCF", "IORWF", "MOVF", "RLF", "RRF", "SUBWF", "XORWF",
];

/// True when `toks[1]` (a BCF/BSF/MOVWF operand) names STATUS, by symbol
/// or by its register address, mirroring the register half of
/// `banking::bank_op_effect`'s own token handling (that function only
/// exposes STATUS-ness bundled with an RP-bit decode; a `BCF/BSF
/// STATUS,0` clearing Carry directly needs the same STATUS check without
/// the RP interpretation, so this narrow slice of the same parsing is
/// duplicated here rather than widening `bank_op_effect`'s own contract).
fn targets_status(toks: &[&str]) -> bool {
    let reg = match toks.get(2) {
        Some(_) => toks.get(1).copied().unwrap_or(""),
        None => {
            let t = toks.get(1).copied().unwrap_or("");
            match t.find(',') {
                Some(i) => &t[..i],
                None => t,
            }
        }
    }
    .trim_end_matches([',', ';', ')'])
    .trim();
    reg == "STATUS"
        || reg
            .strip_prefix("0x")
            .and_then(|h| u16::from_str_radix(h, 16).ok())
            == Some(0x03)
}

/// The `0x..` literal address named by `toks[1]`, when there is one.
fn literal_addr(toks: &[&str]) -> Option<u16> {
    let op = toks.get(1)?.trim_end_matches([',', ';', ')']);
    u16::from_str_radix(op.strip_prefix("0x")?, 16).ok()
}

/// True when the byte-oriented instruction's destination is `f` itself
/// (the trailing `, F` form isel emits, e.g. `RLF 0x20, F`), false when it
/// is `W` (`, W`, e.g. `ANDWF 0x27, W`).
fn dest_is_file(toks: &[&str]) -> bool {
    toks.last().map(|t| t.trim_end_matches(';')) == Some("F")
}

fn classify_insn<'a>(device: &Device, line: &'a str, mne: &'a str, toks: &[&str]) -> Line<'a> {
    let bank = operand_bank(device, mne, toks);
    let addr = literal_addr(toks);
    let base = |reads_w, writes_w, reads_flags, writes_flags, reads_file, writes_file| Insn {
        line,
        mnemonic: mne,
        bank,
        reads_w,
        writes_w,
        reads_flags,
        writes_flags,
        file_addr: addr,
        reads_file,
        writes_file,
        is_skip: SKIP_OPS.contains(&mne),
        is_skip_target: false, // filled in by `classify` once the sequence is known
    };
    if DEST_SELECTABLE.contains(&mne) {
        let reads_w = DEST_SELECTABLE_READS_W.contains(&mne);
        let sets_flags = DEST_SELECTABLE_SETS_FLAGS.contains(&mne);
        let to_file = dest_is_file(toks);
        return Line::Insn(base(reads_w, !to_file, false, sets_flags, true, to_file));
    }
    match mne {
        "CLRF" => Line::Insn(base(false, false, false, true, false, true)),
        "CLRW" => Line::Insn(base(false, true, false, true, false, false)),
        "MOVWF" => Line::Insn(base(true, false, false, false, false, true)),
        "NOP" => Line::Insn(base(false, false, false, false, false, false)),
        "DECFSZ" | "INCFSZ" => {
            let to_file = dest_is_file(toks);
            Line::Insn(base(false, !to_file, false, false, true, to_file))
        }
        "BCF" | "BSF" => {
            let flags = targets_status(toks);
            Line::Insn(base(false, false, flags, flags, !flags, !flags))
        }
        "BTFSC" | "BTFSS" => {
            let flags = targets_status(toks);
            Line::Insn(base(false, false, flags, false, !flags, false))
        }
        "ADDLW" | "SUBLW" => Line::Insn(base(true, true, false, true, false, false)),
        "ANDLW" | "IORLW" | "XORLW" => Line::Insn(base(true, true, false, true, false, false)),
        "MOVLW" => Line::Insn(base(false, true, false, false, false, false)),
        // Everything else, including RETLW reaching here (it shouldn't:
        // the caller classifies it as a terminator first) and a
        // bank-select op reaching schedule's input at all (unusual --
        // ordinary isel output has none yet; only a hand-written
        // `; --- asm start/end ---` block could): opaque, this module's
        // own "unknown is safe" default rather than an assumption.
        _ => Line::Opaque(line),
    }
}

/// Classify every line of `asm`. See `Line`/`Insn` for what's tracked.
pub fn classify<'a>(device: &Device, asm: &'a str) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    let mut in_asm = false;
    for line in asm.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("; --- asm start ---") {
            in_asm = true;
            out.push(Line::AsmBarrier(line));
            continue;
        }
        if trimmed.starts_with("; --- asm end ---") {
            in_asm = false;
            out.push(Line::AsmBarrier(line));
            continue;
        }
        if in_asm {
            out.push(Line::InAsm(line));
            continue;
        }
        let toks: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(mne) = toks.first().copied() else {
            out.push(Line::Blank);
            continue;
        };
        if mne.ends_with(':') {
            out.push(Line::Label(line));
            continue;
        }
        if mne == "org" || mne == "end" || mne.starts_with('.') {
            out.push(Line::Directive(line));
            continue;
        }
        if mne == "CALL" {
            out.push(Line::Call(line));
            continue;
        }
        if TERMINATORS.contains(&mne) {
            out.push(Line::Terminator(line));
            continue;
        }
        out.push(classify_insn(device, line, mne, &toks));
    }
    // Second pass: mark every line right after a skip op as a skip
    // target, an atomic pair the region/hazard model must never split.
    for i in 1..out.len() {
        let prev_is_skip = matches!(&out[i - 1], Line::Insn(p) if p.is_skip);
        if prev_is_skip {
            if let Line::Insn(cur) = &mut out[i] {
                cur.is_skip_target = true;
            }
        }
    }
    out
}

/// Split `lines` (as returned by `classify`) into straight-line regions:
/// the longest runs of consecutive `Line::Insn` entries, broken at every
/// barrier (`is_barrier`). Each region is a half-open `[start, end)`
/// index range into `lines`; barrier lines themselves belong to no
/// region.
pub fn regions(lines: &[Line]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if l.is_barrier() {
            if let Some(s) = start.take() {
                out.push(s..i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(s..lines.len());
    }
    out
}

/// `cur` is never a move candidate unless every one of these holds: not a
/// skip op, not a skip target (issue #6: never move the other half of an
/// atomic pair), and touches neither W nor a flag. The last two together
/// mean moving `cur` past one neighbor can never disturb a W-chain or a
/// flag-chain, since `cur` itself is simply not part of either chain; the
/// only remaining hazard to check per neighbor is a shared file-register
/// address (see `file_collision`).
fn is_move_candidate(cur: &Insn) -> bool {
    cur.bank.is_some()
        && !cur.is_skip
        && !cur.is_skip_target
        && !cur.reads_flags
        && !cur.writes_flags
        && !cur.reads_w
        && !cur.writes_w
}

/// True when `a` and `b` name the same file-register address: swapping
/// them would reorder a read/write against itself (RAW/WAR/WAW), unsound
/// regardless of what `a`/`b` otherwise do. Given `phase1`'s own excursion
/// precondition (`cur.bank != prev.bank`, and a bank is a pure function
/// of the address), this can never actually trigger for the exact
/// neighbor `phase1` calls it with today: same address always means same
/// bank, so a same-address neighbor could never have satisfied the
/// excursion check in the first place. Kept anyway as the real invariant
/// this function is supposed to guarantee, checked directly rather than
/// assumed, and load-bearing the moment any future phase loosens the
/// precondition that currently makes it unreachable.
fn file_collision(a: &Insn, b: &Insn) -> bool {
    a.file_addr.is_some() && a.file_addr == b.file_addr
}

/// Try to swap `lines[i]` with the immediately following instruction
/// (`lines[i + 1]`), reducing an `A, cur(B), A` excursion to `A, A, cur`
/// -- one fewer bank switch, since the two `A` operands are now adjacent
/// and `cur` instead directly precedes whatever needed a switch to `B`
/// anyway. Safe when `next` is not a skip op (swapping into the position
/// right after a skip op would corrupt what that skip actually guards)
/// and the two don't share a file-register address. Returns whether the
/// swap happened.
fn try_sink(lines: &mut [Line], i: usize) -> bool {
    let ok = match (&lines[i], &lines[i + 1]) {
        (Line::Insn(cur), Line::Insn(next)) => !next.is_skip && !file_collision(cur, next),
        _ => false,
    };
    if ok {
        lines.swap(i, i + 1);
    }
    ok
}

/// The mirror of `try_sink`: swap `lines[i]` with the immediately
/// preceding instruction (`lines[i - 1]`), reducing an `A, cur(B), A`
/// excursion to `cur, A, A`. Safe when `prev` is not itself a skip target
/// (swapping something in front of it would land between its skip op and
/// it) and the two don't share a file-register address.
fn try_hoist(lines: &mut [Line], i: usize) -> bool {
    let ok = match (&lines[i - 1], &lines[i]) {
        (Line::Insn(prev), Line::Insn(cur)) => !prev.is_skip_target && !file_collision(cur, prev),
        _ => false,
    };
    if ok {
        lines.swap(i - 1, i);
    }
    ok
}

/// Phase 1 (ADR-027, epic-cc#210): the single hand-verified reorder
/// shape, deliberately narrower than general list scheduling (no
/// lookahead past one neighbor on either side, no multi-instruction
/// bundling, no flag-chain reasoning). For every `Line::Insn` at index
/// `i` strictly inside a region (so both `i - 1` and `i + 1` exist in the
/// same straight-line run `regions` already computed), sandwiched between
/// two neighbors that both need the SAME bank `cur` itself doesn't:
/// try sinking `cur` past its successor first (matching the shape found
/// by hand during epic-cc#210's investigation), falling back to hoisting
/// it past its predecessor when sinking is blocked. Mutates `lines` in
/// place; returns the number of swaps performed.
///
/// Deliberately does NOT capture every hand-traced excursion from the
/// investigation: an excursion instruction that is itself a `MOVWF` (or
/// any op reading W) is never a move candidate here by construction
/// (`is_move_candidate`), even when the actual fix is to move a
/// DIFFERENT, independent instruction earlier instead (the `EPIC_IRQ_
/// Enable` example ADR-027 cites is exactly this shape) -- that needs
/// moving more than one instruction and is explicitly deferred to a
/// later phase, not silently included here.
pub fn phase1(lines: &mut [Line]) -> usize {
    let mut swapped = 0usize;
    for region in regions(lines) {
        if region.len() < 3 {
            continue;
        }
        let mut i = region.start + 1;
        while i + 1 < region.end {
            let excursion = match (&lines[i - 1], &lines[i], &lines[i + 1]) {
                (Line::Insn(prev), Line::Insn(cur), Line::Insn(next)) => {
                    is_move_candidate(cur)
                        && prev.bank.is_some()
                        && prev.bank == next.bank
                        && prev.bank != cur.bank
                }
                _ => false,
            };
            if excursion && (try_sink(lines, i) || try_hoist(lines, i)) {
                swapped += 1;
            }
            i += 1;
        }
    }
    swapped
}

/// Reorder small, provably-independent instruction groups to reduce
/// mid-block bank switches (ADR-027, epic-cc#210). See `phase1` for
/// exactly what this does and does not move.
pub fn schedule(device: &Device, asm: &str) -> String {
    let mut lines = classify(device, asm);
    phase1(&mut lines);
    // `str::lines()` never yields a trailing empty entry for a single
    // final `\n` (or no entry at all for input with none), so joining
    // `lines` back with `\n` and only conditionally appending one more
    // reproduces `asm`'s own trailing-newline-or-not exactly; always
    // appending one after every line (including the last) silently grew
    // isel's actual raw output by a phantom blank line, which
    // `crates/asm` does not parse the same as no trailing line at all
    // (found via the full fuzz corpus, seed 128).
    let mut out = lines.iter().map(Line::raw).collect::<Vec<_>>().join("\n");
    if asm.ends_with('\n') {
        out.push('\n');
    }
    out
}
