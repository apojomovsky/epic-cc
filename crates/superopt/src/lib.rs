//! A small enumerate-and-verify superoptimizer spike (epic-cc#514).
//!
//! A candidate is plain PIC18 assembly text, assembled with the real
//! encoder (`asm::assemble_pic18`, so a malformed candidate just fails to
//! assemble rather than needing its own well-formedness check) and checked
//! by running it on `pic14_sim::Pic18`, the same simulator the rest of the
//! repo already trusts as a behavioral oracle for whole-program XC8
//! differential runs and sim-verified e2e fixtures. A candidate that
//! reproduces the expected output on every case in a fixed case set is
//! "verified" here; this is exhaustive-input checking over a curated
//! set of inputs, not a proof the way an SMT-based superoptimizer would
//! give one. See `docs/41-superopt-spike-findings.md` for the fidelity
//! discussion (what makes the case set trustworthy per target).

use pic14_sim::Pic18;

/// One verification case: memory pokes to apply to a fresh `Pic18` before
/// running (typically STATUS flag bits and/or a poisoned destination byte,
/// to catch a candidate that only works by accident), and a predicate the
/// halted machine must satisfy.
pub struct Case {
    pub pokes: Vec<(usize, u8)>,
    pub check: Box<dyn Fn(&Pic18) -> bool>,
}

/// A candidate instruction sequence, one PIC18 asm source line per entry.
/// Every candidate here is straight-line with no labels: the skip
/// instructions in the alphabet (`BTFSC`/`BTFSS`) skip exactly the next
/// line, which is what the real hardware does, so branch targets never
/// need resolving.
pub type Candidate = Vec<&'static str>;

const MAX_STEPS: usize = 64;

/// Assemble `candidate` plus a trailing `sleep`, run it once per case in
/// `cases`, and return whether every case's predicate held against the
/// halted machine. A candidate whose lines do not assemble (a template
/// producing an invalid operand combination) or that never reaches `sleep`
/// within the step budget counts as not verified, not as an error: the
/// caller is enumerating, most candidates are expected to fail.
pub fn verify(candidate: &Candidate, cases: &[Case]) -> bool {
    let src = candidate_source(candidate);
    let words = match std::panic::catch_unwind(|| asm::assemble_pic18(&src)) {
        Ok(words) if !words.is_empty() => words,
        _ => return false,
    };
    cases.iter().all(|case| run_case(&words, case))
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

fn run_case(words: &[u16], case: &Case) -> bool {
    let mut sim = Pic18::new(words.to_vec());
    for &(addr, val) in &case.pokes {
        sim.ram_mut()[addr] = val;
    }
    sim.run(MAX_STEPS);
    // Never reaching `sleep` (a candidate that loops via a skip
    // instruction, or one whose branch target does not exist) is a
    // rejection, not a hang: MAX_STEPS bounds every run.
    sim.halted() && (case.check)(&sim)
}

/// Breadth-first over `alphabet`: try every length from 1 up to and
/// including `max_len`, return every candidate at the first length with at
/// least one verified hit (not just the first found), so ties are visible.
/// Silences panic output for the duration: most candidates in a brute-force
/// sweep are expected to fail to assemble, and that is not worth a stack
/// trace per attempt.
pub fn shortest(alphabet: &[&'static str], cases: &[Case], max_len: usize) -> Vec<Candidate> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = (1..=max_len)
        .map(|len| search_length(alphabet, cases, len))
        .find(|hits| !hits.is_empty())
        .unwrap_or_default();
    std::panic::set_hook(prev_hook);
    result
}

fn search_length(alphabet: &[&'static str], cases: &[Case], len: usize) -> Vec<Candidate> {
    let mut hits = Vec::new();
    let mut idx = vec![0usize; len];
    loop {
        let candidate: Candidate = idx.iter().map(|&i| alphabet[i]).collect();
        if verify(&candidate, cases) {
            hits.push(candidate);
        }
        if !advance(&mut idx, alphabet.len()) {
            break;
        }
    }
    hits
}

/// Odometer-style increment over `idx`, index 0 the fastest-spinning digit.
/// Returns false once every combination of length `idx.len()` over a base
/// of `base` symbols has been visited.
fn advance(idx: &mut [usize], base: usize) -> bool {
    for slot in idx.iter_mut() {
        *slot += 1;
        if *slot < base {
            return true;
        }
        *slot = 0;
    }
    false
}

/// PIC18 access-bank STATUS register's physical RAM address (`0xFD8`),
/// matching `Pic18::status_addr`'s own `resolve_f(0, 0xD8)` (private to the
/// sim crate, so this is a second source of truth for the same constant;
/// checked indirectly, against a real `ADDLW`'s flag output, by
/// `status_z_bit_reflects_a_real_addlw` and
/// `status_c_bit_reflects_a_real_addlw` below).
pub const STATUS_ADDR: usize = 0xFD8;
pub const STATUS_Z_BIT: u8 = 0x04;
pub const STATUS_C_BIT: u8 = 0x01;

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 0 (epic-cc#514): confirm `STATUS_ADDR`/`STATUS_Z_BIT` actually
    /// track the flag an instruction sets, not just the address this crate
    /// happens to poke and read back. `0x00 + 0x00` sets Z; `0x01 + 0x00`
    /// clears it. If the sim's flag computation or address were wrong,
    /// candidates could "verify" against a Z bit that never reflects a
    /// real comparison.
    #[test]
    fn status_z_bit_reflects_a_real_addlw() {
        for (imm, want_z) in [("0x00", true), ("0x01", false)] {
            let words = asm::assemble_pic18(&format!("movlw {imm}\naddlw 0x00\nsleep\n"));
            let mut sim = Pic18::new(words);
            sim.run(MAX_STEPS);
            assert!(sim.halted());
            let z_set = sim.ram()[STATUS_ADDR] & STATUS_Z_BIT != 0;
            assert_eq!(z_set, want_z, "ADDLW {imm}, 0x00: Z bit");
        }
    }

    /// Same check for the carry flag, since the shift-chain target
    /// (epic-cc#505) rotates through `C`: `0xFF + 0x01` carries, `0x01 +
    /// 0x01` does not.
    #[test]
    fn status_c_bit_reflects_a_real_addlw() {
        for (imm, want_c) in [("0xFF", true), ("0x01", false)] {
            let words = asm::assemble_pic18(&format!("movlw {imm}\naddlw 0x01\nsleep\n"));
            let mut sim = Pic18::new(words);
            sim.run(MAX_STEPS);
            assert!(sim.halted());
            let c_set = sim.ram()[STATUS_ADDR] & STATUS_C_BIT != 0;
            assert_eq!(c_set, want_c, "ADDLW {imm}, 0x01: C bit");
        }
    }

    /// `BTFSS`/`BTFSC` skip the literal next line with no label, which is
    /// the mechanism `shortest`'s alphabet relies on for conditional
    /// materialization. Confirm it against the sim directly before trusting
    /// any search result built on it.
    #[test]
    fn btfss_skips_exactly_the_next_line() {
        // Z=1 (movlw 0x00 / addlw 0x00): BTFSS on the Z bit skips the
        // following MOVLW 0xAA, so W stays 0x00 through to SLEEP.
        let words =
            asm::assemble_pic18("movlw 0x00\naddlw 0x00\nbtfss 0xFD8,2,A\nmovlw 0xAA\nsleep\n");
        let mut sim = Pic18::new(words);
        sim.run(MAX_STEPS);
        assert!(sim.halted());
        assert_eq!(sim.w(), 0x00, "BTFSS should have skipped the MOVLW 0xAA");
    }

    /// The odometer must actually enumerate every combination once, in
    /// particular the all-zero and all-max indices (the easiest ones to
    /// get an off-by-one wrong on).
    #[test]
    fn advance_visits_every_combination_exactly_once() {
        let mut idx = vec![0usize; 2];
        let mut seen = std::collections::HashSet::new();
        loop {
            assert!(seen.insert(idx.clone()), "revisited {idx:?}");
            if !advance(&mut idx, 3) {
                break;
            }
        }
        assert_eq!(seen.len(), 9, "3^2 combinations of length 2");
        assert!(seen.contains(&vec![0, 0]));
        assert!(seen.contains(&vec![2, 2]));
    }

    /// A candidate that never reaches `sleep` (an infinite skip/no-op
    /// combination is not possible with this alphabet, but an empty
    /// program or a program that traps is) must fail verification, not
    /// hang `verify` or panic the caller.
    #[test]
    fn a_non_halting_candidate_does_not_verify() {
        let candidate: Candidate = vec!["nop"];
        // Even NOP reaches the trailing `sleep` `verify` appends, so build
        // the case set to fail regardless: a `check` that always returns
        // false confirms a *verified halt with a wrong answer* is rejected
        // exactly like a hang would be, without needing an instruction
        // that actually loops forever.
        let cases = vec![Case {
            pokes: vec![],
            check: Box::new(|_| false),
        }];
        assert!(!verify(&candidate, &cases));
    }
}
