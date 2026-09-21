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

/// One verification case: an entry `W` value (`Pic18::set_w`, since `W` is
/// a CPU register, not a RAM byte `pokes` could reach), memory pokes to
/// apply to a fresh `Pic18` before running, the RAM addresses the
/// candidate is allowed to write to, and a predicate the halted machine
/// must satisfy. `allowed_changes` is owned (`Vec`, not `&'static
/// [usize]`), matching `pokes`: a caller building cases from a runtime
/// parameter (a target address, e.g. epic-cc#520's generalization) cannot
/// produce a `'static` slice from it.
///
/// `run_case` rejects any RAM byte outside `allowed_changes` that changed
/// from its post-poke value, `STATUS_ADDR` and `W` excepted: both are
/// treated as scratch, never part of the correctness contract these
/// targets search for. See `docs/41-superopt-spike-findings.md` for the
/// design history (two review rounds found real soundness gaps here) and
/// the caveat this scratch exception implies for a wider alphabet.
pub struct Case {
    pub entry_w: u8,
    pub pokes: Vec<(usize, u8)>,
    pub allowed_changes: Vec<usize>,
    pub check: Box<dyn Fn(&Pic18) -> bool>,
}

/// A candidate instruction sequence, one PIC18 asm source line per entry.
/// Every candidate here is straight-line with no labels: the skip
/// instructions in the alphabet (`BTFSC`/`BTFSS`) skip exactly the next
/// line, which is what the real hardware does, so branch targets never
/// need resolving.
pub type Candidate = Vec<&'static str>;

const MAX_STEPS: usize = 64;

/// The non-zero sentinel `run_case` poisons RAM with before applying a
/// case's `pokes`. Any value works as long as it is not `0x00` (RAM's own
/// reset value) and callers do not rely on an unpoked byte reading as
/// this specific value; no current alphabet or check does.
const POISON: u8 = 0xA5;

/// Assemble `candidate` plus a trailing `sleep`, run it once per case in
/// `cases`, and return whether every case's predicate held against the
/// machine after it genuinely executed `sleep`. A candidate whose lines do
/// not assemble (a template producing an invalid operand combination), that
/// hits an opcode the simulator does not model, or that never reaches
/// `sleep` within the step budget counts as not verified, not as an error:
/// the caller is enumerating, most candidates are expected to fail. Both
/// the assembler and the simulator run under `catch_unwind`, since both are
/// allowed to panic on this crate's own scaffolding (the curated alphabets
/// stay inside what each supports; a future wider alphabet may not).
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
    // The appended `sleep` (see `candidate_source`) is always the last
    // word; its own byte address is where a genuine halt must land. `Pic18`
    // also flags `halted` when `pc` merely runs off the end of `prog` (a
    // candidate whose last line is a taken `BTFSC`/`BTFSS` that skips past
    // the appended `sleep` entirely), which is a different condition and
    // must not verify: the skipped instruction's effect on a real target is
    // whatever comes next in memory, not defined by this candidate at all.
    let sleep_addr = ((words.len() - 1) * 2) as u32;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut sim = Pic18::new(words.to_vec());
        // Poison before pokes: a poke's own value always wins for its own
        // address, and every other byte starts at a value no legitimate
        // write coincides with, so a write that happens to store zero is
        // still visible as a change (see `Case`'s doc for why this matters).
        sim.ram_mut().fill(POISON);
        sim.set_w(case.entry_w);
        for &(addr, val) in &case.pokes {
            sim.ram_mut()[addr] = val;
        }
        let before = *sim.ram();
        sim.run(MAX_STEPS);
        let genuinely_halted = sim.halted() && sim.pc() == sleep_addr;
        genuinely_halted
            && no_unexpected_clobber(&before, sim.ram(), &case.allowed_changes)
            && (case.check)(&sim)
    }));
    outcome.unwrap_or(false)
}

/// Every RAM byte that changed between `before` and `after` is either in
/// `allowed` or is `STATUS_ADDR` (flag side effects from ALU/skip
/// instructions are expected and never what a candidate is judged on).
/// Anything else that moved is a clobber this candidate must not pass on.
///
/// Builds `expected` (a copy of `before` with only `STATUS_ADDR` and
/// `allowed`'s addresses overwritten from `after`) and compares it to
/// `after` with one array equality, instead of a per-byte `enumerate` plus
/// an `allowed.contains` linear scan at every one of 4096 bytes: the
/// latter measured 6-24x slower in epic-cc#521's review for no benefit,
/// since the two express the same predicate.
fn no_unexpected_clobber(before: &[u8; 4096], after: &[u8; 4096], allowed: &[usize]) -> bool {
    let mut expected = *before;
    expected[STATUS_ADDR] = after[STATUS_ADDR];
    for &addr in allowed {
        expected[addr] = after[addr];
    }
    &expected == after
}

/// Restores the previous panic hook on drop, including on unwind: without
/// this, a panic that escapes `search_length` despite `verify`'s own
/// `catch_unwind` (a bug in this crate's search logic itself, not in a
/// candidate) would leave the silencing hook installed for the rest of the
/// process, swallowing every later panic's message, in any other test in
/// the same binary, silently.
struct PanicHookGuard(Option<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>>);

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            std::panic::set_hook(hook);
        }
    }
}

/// The panic hook is process-global state: two `shortest` calls racing
/// from different test threads (epic-cc#520 was the first target file to
/// call `shortest` from more than one `#[test]` in the same binary) can
/// interleave `take_hook`/`set_hook`, so one call's `Drop` guard restores
/// the *other* call's silencing hook instead of the real one, permanently
/// silencing every later panic in the process, including a genuine
/// assertion failure's message (found in epic-cc#520's review). Serializes
/// every `shortest` call on this lock so the hook swap is never observed
/// mid-flight by another thread.
static HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Breadth-first over `alphabet`: try every length from 1 up to and
/// including `max_len`, return every candidate at the first length with at
/// least one verified hit (not just the first found), so ties are visible.
/// Silences panic output for the duration: most candidates in a brute-force
/// sweep are expected to fail to assemble, and that is not worth a stack
/// trace per attempt.
pub fn shortest(alphabet: &[&'static str], cases: &[Case], max_len: usize) -> Vec<Candidate> {
    let _lock = HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _guard = PanicHookGuard(Some(std::panic::take_hook()));
    std::panic::set_hook(Box::new(|_| {}));
    (1..=max_len)
        .map(|len| search_length(alphabet, cases, len))
        .find(|hits| !hits.is_empty())
        .unwrap_or_default()
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

    /// A candidate that never reaches `sleep` must fail verification, not
    /// hang `verify` or panic the caller. Uses a real infinite loop (a
    /// label and an unconditional branch to itself, the one place this
    /// crate's tests step outside the label-free alphabet it searches
    /// over), bounded only by `MAX_STEPS`, so this exercises the actual
    /// step-budget rejection path rather than a `check` engineered to fail
    /// for an unrelated reason.
    #[test]
    fn a_looping_candidate_does_not_verify() {
        let candidate: Candidate = vec!["loop:", "bra loop"];
        let cases = vec![Case {
            entry_w: 0,
            pokes: vec![],
            allowed_changes: vec![],
            check: Box::new(|_| true),
        }];
        assert!(!verify(&candidate, &cases));
    }

    /// `Case::entry_w` must reach the machine's `W` register, not just sit
    /// unused: the bug this crate shipped with once (epic-cc#514's review)
    /// looped over candidate `W` values without ever applying them, so
    /// every candidate that only worked by accident on `W == 0` (the
    /// simulator's always-zero reset) silently "verified".
    #[test]
    fn entry_w_reaches_the_machine() {
        let candidate: Candidate = vec!["movwf 0x020,A"];
        for w in [0x00u8, 0x2A, 0xFF] {
            let cases = vec![Case {
                entry_w: w,
                pokes: vec![(0x020, 0x55)],
                allowed_changes: vec![0x020],
                check: Box::new(move |sim: &Pic18| sim.ram()[0x020] == w),
            }];
            assert!(
                verify(&candidate, &cases),
                "entry_w {w:#04x} did not reach W"
            );
        }
    }

    /// A candidate whose last instruction is a taken skip runs off the end
    /// of `prog` (past the appended `sleep`) instead of executing it.
    /// `Pic18` still reports `halted()` for that (any `pc` past the end of
    /// `prog` halts), so `run_case` must reject it on the `pc` address
    /// check, not just trust `halted()` (epic-cc#514's review, finding 7).
    #[test]
    fn a_trailing_taken_skip_does_not_verify_even_though_the_sim_halts() {
        // Z=1 (entry_w unused here): BTFSS on Z skips the appended
        // `sleep` itself, running pc off the end of the program.
        let candidate: Candidate = vec!["movlw 0x00", "addlw 0x00", "btfss 0xFD8,2,A"];
        let cases = vec![Case {
            entry_w: 0,
            pokes: vec![],
            allowed_changes: vec![],
            check: Box::new(|_| true), // even an always-true check must not save it
        }];
        assert!(!verify(&candidate, &cases));
    }

    /// The clobber check itself (epic-cc#521): a candidate that produces
    /// the right destination value while also writing an unrelated byte
    /// must not verify, even though `check` only ever looks at the
    /// destination.
    #[test]
    fn a_candidate_that_clobbers_an_unrelated_byte_does_not_verify() {
        // `CLRF 0x021,A` zeroes a byte the case set never allows to change,
        // then `MOVWF 0x020,A` correctly stores W to the actual destination.
        let candidate: Candidate = vec!["clrf 0x021,A", "movwf 0x020,A"];
        let cases = vec![Case {
            entry_w: 0x2A,
            pokes: vec![(0x020, 0x55), (0x021, 0x55)],
            allowed_changes: vec![0x020], // 0x021 is deliberately not listed
            check: Box::new(|sim: &Pic18| sim.ram()[0x020] == 0x2A),
        }];
        assert!(!verify(&candidate, &cases));
    }

    /// The same candidate verifies once the clobbered byte is declared, so
    /// the previous test is checking the clobber gate specifically, not
    /// some other accidental failure.
    #[test]
    fn declaring_the_clobbered_byte_lets_it_verify() {
        let candidate: Candidate = vec!["clrf 0x021,A", "movwf 0x020,A"];
        let cases = vec![Case {
            entry_w: 0x2A,
            pokes: vec![(0x020, 0x55), (0x021, 0x55)],
            allowed_changes: vec![0x020, 0x021],
            check: Box::new(|sim: &Pic18| sim.ram()[0x020] == 0x2A),
        }];
        assert!(verify(&candidate, &cases));
    }

    /// The clobber this crate's own review found a hole for: a `CLRF` on a
    /// byte the case never pokes at all. `Pic18::new` zeroes RAM, so an
    /// unpoked byte's before-value and this write's after-value are both
    /// `0x00`, an unpoisoned before/after comparison sees no change, and
    /// the clobber passes silently. `run_case`'s RAM poisoning (this fix)
    /// makes every unpoked byte start at a value no legitimate write
    /// coincides with, so the same write is now visible.
    #[test]
    fn a_clobber_of_an_unpoked_byte_does_not_verify() {
        // 0x022 is never poked, so its pre-poison, pre-fix value would
        // have been 0x00, identical to what CLRF leaves it at.
        let candidate: Candidate = vec!["clrf 0x022,A", "movwf 0x020,A"];
        let cases = vec![Case {
            entry_w: 0x2A,
            pokes: vec![(0x020, 0x55)],
            allowed_changes: vec![0x020],
            check: Box::new(|sim: &Pic18| sim.ram()[0x020] == 0x2A),
        }];
        assert!(!verify(&candidate, &cases));
    }

    /// `shortest` from more than one thread at once must neither deadlock
    /// nor corrupt a search's result (epic-cc#520's review: the panic-hook
    /// swap `shortest` does internally is process-global state, and two
    /// concurrent calls racing on it previously left the silencing hook
    /// installed permanently, which this test cannot observe directly
    /// without risking the std test harness's own `#[should_panic]`
    /// hook, so it stays scoped to deadlock-freedom and per-call
    /// correctness: `HOOK_LOCK` serializing the hook swap is the actual
    /// fix, this is a smoke test that using it works at all).
    #[test]
    fn concurrent_shortest_calls_do_not_deadlock_or_corrupt_results() {
        let handles: Vec<_> = (0..6)
            .map(|_| {
                std::thread::spawn(|| {
                    let cases = vec![Case {
                        entry_w: 0,
                        pokes: vec![(0x020, 0x55)],
                        allowed_changes: vec![0x020],
                        check: Box::new(|sim: &Pic18| sim.ram()[0x020] == 0x00),
                    }];
                    shortest(&["clrf 0x020,A", "setf 0x020,A"], &cases, 1)
                })
            })
            .collect();
        for h in handles {
            let hits = h.join().expect("shortest panicked or deadlocked");
            assert_eq!(hits, vec![vec!["clrf 0x020,A"]]);
        }
    }
}
