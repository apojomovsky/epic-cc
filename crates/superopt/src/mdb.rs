//! MDB (MPLAB SIM) cross-check batch builder (epic-cc#712).
//!
//! The in-sim `verify` already proves each landed sequence over its full
//! domain with full-RAM clobber detection; what it cannot prove is that
//! the simulator itself models the silicon correctly. This module emits
//! one self-contained program per case chunk that replays the test's own
//! cases on real MPLAB SIM and reads the results back with `x /1xbr`, so
//! a second, independent executor judges the same domain. Division of
//! labor: sim owns exhaustive domains and no-clobber over 4 KB; MDB owns
//! value agreement on every allowed address plus `W`, `STATUS`, and
//! canary guards around the touched regions.

use crate::{Candidate, Case, STATUS_ADDR};
use pic14_sim::Pic18;

pub const WREG_ADDR: usize = 0xFE8;
/// Device-selection proof: `TRISB` is fully implemented (all 8 pins) with
/// POR `0xFF` on the 4550, in both the model and MPLAB SIM. `TRISA` looks
/// equivalent but is not: RA6/RA7 are oscillator pins, and the SIM returns
/// its unimplemented bit 6 set, so it reads `0x7F` against the model's
/// `0xFF`. A proof register must be fully implemented, not just POR-known.
pub const PROOF_ADDR: usize = 0xF93;
pub const PROOF_POR: u8 = 0xFF;
pub const POISON: u8 = 0xA5;
const SFR_FLOOR: usize = 0xF80;
const FLASH_WORDS: usize = 16384;
const MAX_STEPS: usize = 64;

/// One emitted program plus its readback contract: `reads` are the
/// addresses to dump with `x /1xbr` in order, `expected` the byte each
/// must hold (proof first, then guards, then per-case `W`, `STATUS`,
/// allowed). `tags` names each read's source in order (`proof`,
/// `guard`, or the case lane), so a mismatch log names the failing
/// lane without re-running. `stepi` is the instruction count plus
/// margin: overshoot lands in the side-effect-free spin, so the
/// session stays deterministic when the wait expires mid-program.
pub struct Batch {
    pub src: String,
    pub reads: Vec<usize>,
    pub expected: Vec<u8>,
    pub tags: Vec<String>,
    pub stepi: usize,
}

/// Suffixes a candidate-local label with the case index so one candidate
/// can replay per case in a single program: without it the second
/// repetition redefines the label and the batch does not assemble.
/// Label-free candidates (every spec before the bitmask lanes) pass
/// through unchanged.
fn gensym(line: &str, case_idx: usize, labels: &[&str]) -> String {
    let trimmed = line.trim();
    if let Some(name) = trimmed.strip_suffix(':') {
        if !name.contains(char::is_whitespace) {
            return format!("{name}_c{case_idx}:");
        }
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let op = parts.next().unwrap_or("");
    let operand = parts.next().unwrap_or("").trim();
    if matches!(
        op.to_ascii_lowercase().as_str(),
        "bra" | "bz" | "bnz" | "goto" | "rcall" | "call"
    ) && labels.contains(&operand)
    {
        return format!("{op} {operand}_c{case_idx}");
    }
    // A table-address literal (`MOVLW LOW(tbl)`) names the same
    // case-local label: suffix it like a branch target so each
    // repetition addresses its own table (epic-cc#832).
    let mut out = line.to_string();
    for label in labels {
        for lit in ["LOW", "HIGH", "UPPER"] {
            out = out.replace(
                &format!("{lit}({label})"),
                &format!("{lit}({label}_c{case_idx})"),
            );
        }
    }
    out
}

/// Replay `cases` against `candidate` on hardware. `out_base` is the
/// first output-table byte (banked GPR, e.g. 0x100); each case occupies
/// one `W` slot, one `STATUS` slot, then one slot per allowed address.
pub fn build_batch(candidate: &Candidate, cases: &[Case], out_base: usize) -> Batch {
    // Labels this candidate defines: branch targets get the case index
    // below so repetitions never redefine one another.
    let labels: Vec<&str> = candidate
        .iter()
        .filter_map(|l| {
            let name = l.trim().strip_suffix(':')?;
            (!name.contains(char::is_whitespace)).then_some(name)
        })
        .collect();

    let mut src = String::from("goto start\nstart:\n");
    let mut reads = vec![PROOF_ADDR];
    let mut expected = vec![PROOF_POR];
    let mut tags = vec![String::from("proof")];
    let mut guards: Vec<usize> = Vec::new();
    // A guard asserts its byte still reads poison: it must never cover
    // an address a case pokes (every replay overwrites it), only
    // neighbours the candidate must leave alone. Shift specs never trip
    // this because their pokes sit inside `allowed_changes`.
    let poked: Vec<usize> = cases
        .iter()
        .flat_map(|c| c.pokes.iter().map(|(a, _)| *a))
        .collect();
    for case in cases {
        for &addr in &case.allowed_changes {
            for g in [addr.wrapping_sub(1), addr + 1] {
                if g < SFR_FLOOR
                    && !case.allowed_changes.contains(&g)
                    && !poked.contains(&g)
                    && !guards.contains(&g)
                {
                    guards.push(g);
                }
            }
        }
    }
    guards.sort_unstable();
    let slots = 2 + cases[0].allowed_changes.len();
    let table_end = out_base + cases.len() * slots;
    // The 4550 has 2 KB of RAM (0x000-0x7FF): a table spilling past it
    // silently reads back zero. Fail here, not in a mystery mismatch.
    assert!(
        table_end < 0x7F0,
        "mdb: batch table ends at 0x{table_end:03X}, past RAM"
    );
    for g in [out_base.wrapping_sub(1), table_end] {
        if !guards.contains(&g) {
            guards.push(g);
        }
    }
    src.push_str("movlw 0xA5\n");
    for &g in &guards {
        // Access-bank GPR is 0x000-0x07F only: 0x080-0x0FF with the
        // access bit aliases the SFR region, so those guards go through
        // `movff` like every other banked address.
        if g < 0x80 {
            src.push_str(&format!("movwf 0x{g:03X},A\n"));
        } else {
            src.push_str(&format!("movff 0xFE8,0x{g:03X}\n"));
        }
    }
    for (i, case) in cases.iter().enumerate() {
        for &(addr, val) in &case.pokes {
            src.push_str(&format!("movlw 0x{val:02X}\n"));
            // Same access-bank rule as the guards: banked GPR pokes go
            // through `movff` straight from `W`, which it preserves.
            if addr < 0x80 || addr >= SFR_FLOOR {
                src.push_str(&format!("movwf 0x{addr:03X},A\n"));
            } else {
                src.push_str(&format!("movff 0xFE8,0x{addr:03X}\n"));
            }
        }
        src.push_str(&format!("movlw 0x{:02X}\n", case.entry_w));
        for line in candidate {
            src.push_str(&gensym(line, i, &labels));
            src.push('\n');
        }
        let slot = out_base + i * slots;
        src.push_str(&format!("movff 0xFE8,0x{slot:03X}\n"));
        src.push_str(&format!("movff 0xFD8,0x{:03X}\n", slot + 1));
        for (j, &addr) in case.allowed_changes.iter().enumerate() {
            src.push_str(&format!("movff 0x{addr:03X},0x{:03X}\n", slot + 2 + j));
        }
    }
    src.push_str("spin:\ngoto spin\n");
    for &g in &guards {
        reads.push(g);
        expected.push(POISON);
        tags.push(String::from("guard"));
    }
    for (i, case) in cases.iter().enumerate() {
        let slot = out_base + i * slots;
        let words = asm::assemble_pic18(&candidate_source(candidate));
        let vals = run_expected(&words, case).expect("spec case must verify in-sim");
        let tag = format!("case{i}");
        reads.push(slot);
        reads.push(slot + 1);
        expected.push(vals.0);
        expected.push(vals.1);
        tags.push(tag.clone());
        tags.push(tag.clone());
        for (j, &v) in vals.2.iter().enumerate() {
            reads.push(slot + 2 + j);
            expected.push(v);
            tags.push(tag.clone());
        }
    }
    let words = asm::assemble_pic18(&src);
    assert!(
        words.len() < FLASH_WORDS,
        "mdb: batch of {} cases is {} words, exceeds 18F4550 flash",
        cases.len(),
        words.len()
    );
    let instrs = src
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_end().ends_with(':'))
        .count();
    Batch {
        src,
        reads,
        expected,
        tags,
        stepi: instrs + 2000,
    }
}

fn candidate_source(candidate: &Candidate) -> String {
    let mut src = String::new();
    for line in candidate {
        src.push_str(line);
        src.push('\n');
    }
    src.push_str("sleep\n");
    src
}

/// Expected `(W, STATUS, allowed...)` for one case from the in-tree
/// simulator, mirroring `verify`'s setup exactly (poison, pokes, entry
/// `W`). `None` when the candidate does not genuinely halt, which the
/// batch must never be built from.
fn run_expected(words: &[u16], case: &Case) -> Option<(u8, u8, Vec<u8>)> {
    let sleep_addr = ((words.len() - 1) * 2) as u32;
    let mut sim = Pic18::new(words.to_vec());
    sim.ram_mut().fill(POISON);
    sim.set_w(case.entry_w);
    for &(addr, val) in &case.pokes {
        sim.ram_mut()[addr] = val;
    }
    sim.run(MAX_STEPS);
    if !(sim.halted() && sim.pc() == sleep_addr) {
        return None;
    }
    let mut allowed = Vec::new();
    for &addr in &case.allowed_changes {
        allowed.push(sim.ram()[addr]);
    }
    Some((sim.w(), sim.ram()[STATUS_ADDR], allowed))
}

/// Parse `x /1xbr` readback: after each echoed `x /1xbr 0xAAA` command,
/// the next bare-hex line is its value. Anything else (`Stop at`,
/// `address:`, blank) is noise. Strict: every requested address must
/// appear exactly once, in order.
pub fn parse_xbr(text: &str, reads: &[usize]) -> Result<Vec<u8>, String> {
    let mut values = Vec::new();
    let mut pending: Option<usize> = None;
    let mut want = reads.iter().peekable();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(addr_txt) = line.strip_prefix("x /1xbr ") {
            let addr = usize::from_str_radix(addr_txt.trim_start_matches("0x"), 16)
                .map_err(|_| format!("mdb: unparsable echo {line:?}"))?;
            if Some(&addr) != want.peek().copied() {
                return Err(format!("mdb: unexpected echo {line:?}"));
            }
            pending = Some(addr);
            continue;
        }
        if pending.is_none() || line.is_empty() {
            continue;
        }
        if line.chars().all(|c| c.is_ascii_hexdigit()) && line.len() <= 4 {
            let v = u8::from_str_radix(line, 16)
                .map_err(|_| format!("mdb: value out of byte range: {line:?}"))?;
            values.push(v);
            want.next();
            pending = None;
        }
    }
    if values.len() != reads.len() {
        return Err(format!(
            "mdb: got {} of {} readbacks",
            values.len(),
            reads.len()
        ));
    }
    Ok(values)
}

/// One readback byte that disagrees with the simulator, carrying the
/// lane tag of its read so a sweep log names every failing opcode lane.
pub struct Mismatch {
    pub index: usize,
    pub addr: usize,
    pub tag: String,
    pub got: u8,
    pub want: u8,
}

/// Every disagreeing byte, in readback order. Length mismatch is an
/// `Err`; value mismatches collect instead of stopping at the first, so
/// one log names every failing lane of a sweep shard.
pub fn diff_batch(batch: &Batch, values: &[u8]) -> Result<Vec<Mismatch>, String> {
    if values.len() != batch.expected.len() {
        return Err(format!(
            "mdb: got {} bytes, expected {}",
            values.len(),
            batch.expected.len()
        ));
    }
    let mut out = Vec::new();
    for (i, (&addr, (&got, &want))) in batch
        .reads
        .iter()
        .zip(values.iter().zip(batch.expected.iter()))
        .enumerate()
    {
        if got != want {
            let tag = batch.tags.get(i).cloned().unwrap_or_default();
            out.push(Mismatch {
                index: i,
                addr,
                tag,
                got,
                want,
            });
        }
    }
    Ok(out)
}

/// Byte-for-byte diff of hardware readback against sim expectation.
/// Any length, address-order, or value mismatch fails: there is no
/// "close enough" for an execution oracle. Reports every mismatch
/// grouped by lane tag with the first details inline, so a sweep shard
/// log names each failing opcode lane explicitly.
pub fn compare_batch(batch: &Batch, values: &[u8]) -> Result<(), String> {
    let mismatches = diff_batch(batch, values)?;
    if mismatches.is_empty() {
        return Ok(());
    }
    let mut lanes: Vec<(&str, usize)> = Vec::new();
    for m in &mismatches {
        match lanes.iter_mut().find(|(t, _)| *t == m.tag) {
            Some(slot) => slot.1 += 1,
            None => lanes.push((m.tag.as_str(), 1)),
        }
    }
    let mut msg = format!(
        "mdb: {} readback(s) disagree with sim across {} lane(s)",
        mismatches.len(),
        lanes.len()
    );
    for (tag, n) in &lanes {
        msg.push_str(&format!("\nlane {tag}: {n} mismatch(es)"));
    }
    for m in mismatches.iter().take(8) {
        msg.push_str(&format!(
            "\n0x{:03X} [{}] (readback {}): got 0x{:02X}, sim says 0x{:02X}",
            m.addr, m.tag, m.index, m.got, m.want
        ));
    }
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specs;

    #[test]
    fn batch_assembles_and_fits_flash() {
        let cases = specs::shift_left_4_cases();
        let batch = build_batch(&specs::shift_left_4_candidate(), &cases, 0x100);
        let words = asm::assemble_pic18(&batch.src);
        assert!(!words.is_empty());
        assert_eq!(batch.reads.len(), batch.expected.len());
        assert_eq!(batch.expected[0], PROOF_POR);
    }

    #[test]
    fn to_hex_matches_gpasm_byte_layout() {
        let words = asm::assemble_pic18("goto start\nstart:\nmovlw 0x0C\nspin:\ngoto spin\n");
        let hex = asm::to_hex(&words);
        let mut lines = hex.lines();
        assert_eq!(lines.next().unwrap(), ":020000040000FA");
        let rec = lines.next().unwrap();
        let nbytes = usize::from_str_radix(&rec[1..3], 16).unwrap();
        assert_eq!(nbytes, 2 * words.len());
        assert_eq!(&rec[3..9], "000000");
        let body = &rec[9..9 + 2 * nbytes];
        for (i, &w) in words.iter().enumerate() {
            assert_eq!(
                u8::from_str_radix(&body[4 * i..4 * i + 2], 16).unwrap(),
                (w & 0xFF) as u8
            );
            assert_eq!(
                u8::from_str_radix(&body[4 * i + 2..4 * i + 4], 16).unwrap(),
                (w >> 8) as u8
            );
        }
    }

    #[test]
    fn parse_xbr_reads_spike_shaped_output() {
        let text = "x /1xbr 0x20\nStop at\n\taddress:0x12\nc0   \n\nx /1xbr 0xFD8\n00\n";
        assert_eq!(parse_xbr(text, &[0x20, 0xFD8]).unwrap(), vec![0xC0, 0x00]);
    }

    #[test]
    fn parse_xbr_rejects_short_readback() {
        let text = "x /1xbr 0x20\nc0\n";
        assert!(parse_xbr(text, &[0x20, 0xFD8]).is_err());
    }

    #[test]
    fn compare_batch_catches_mutation() {
        let cases = specs::shift_left_4_cases();
        let batch = build_batch(&specs::shift_left_4_candidate(), &cases, 0x100);
        let mut values = batch.expected.clone();
        values[10] ^= 0xFF;
        assert!(compare_batch(&batch, &values).is_err());
        assert!(compare_batch(&batch, &batch.expected).is_ok());
    }

    #[test]
    fn gensym_uniquifies_branch_labels_per_case() {
        let candidate: Candidate = vec![
            "decfsz 0x020,W,A",
            "bra lane_skip",
            "bsf 0x021,2,A",
            "lane_skip:",
        ];
        let cases = specs::bitmask_eq1_pr();
        let batch = build_batch(&candidate, &cases[..2], 0x100);
        assert!(batch.src.contains("lane_skip_c0:"));
        assert!(batch.src.contains("lane_skip_c1:"));
        assert!(batch.src.contains("bra lane_skip_c0"));
        assert!(!batch.src.contains("lane_skip:"));
        let words = asm::assemble_pic18(&batch.src);
        assert!(!words.is_empty());
    }

    #[test]
    fn gensym_uniquifies_label_literals_per_case() {
        // A `LOW(label)` table address names the same case-local label
        // as a branch target: each repetition must address its own
        // table (epic-cc#832).
        let candidate: Candidate = vec![
            "bra tab_end",
            "tab:",
            "db 0x34, 0x12",
            "tab_end:",
            "movlw LOW(tab)",
            "movwf 0x040,A",
        ];
        let cases = specs::bitmask_eq1_pr();
        let batch = build_batch(&candidate, &cases[..2], 0x100);
        assert!(batch.src.contains("tab_c0:"));
        assert!(batch.src.contains("tab_c1:"));
        assert!(batch.src.contains("LOW(tab_c0)"));
        assert!(batch.src.contains("LOW(tab_c1)"));
        assert!(!batch.src.contains("LOW(tab)"));
        let words = asm::assemble_pic18(&batch.src);
        assert!(!words.is_empty());
    }
}
