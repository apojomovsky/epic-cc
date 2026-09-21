# 41: Superoptimizer spike findings (epic-cc#514)

Spike, not a production commitment: can a small, exhaustive-search
superoptimizer, verified against the existing `Pic18` simulator instead of
an SMT solver, find shorter instruction sequences than `isel-pic18` for a
handful of small, pure, fixed-contract primitives? Prompted by the density
profiling work (epic-cc#500 and its lineage): two of the ranked sinks,
bool-materialization (#501) and the shift-chain (#505), are exactly the
shape superoptimization is good at, small fixed windows with no external
state, and each already has a hand-derived baseline to beat or confirm
optimal.

The tool lives in `crates/superopt`. It reads `crates/asm`'s encoder
(`assemble_pic18`, so a malformed candidate fails to assemble rather than
needing its own syntax checker) and `crates/sim`'s `Pic18` simulator as the
equivalence oracle. It does not touch `crates/isel-pic18`.

**This document went through one internal review round before merge.** A
first draft of the search reported false-positive results on 3 of 5
published sequences because the entry `W` register was never actually
seeded (see "A soundness bug the review caught" below); the numbers and
candidates in this document are all post-fix, re-run, and independently
re-checked.

## Step 0: is the simulator a faithful oracle for this?

`Pic18` is already used as a behavioral gate elsewhere (XC8-differential
whole-program runs, sim-verified e2e fixtures), but that is a different,
weaker claim than what this spike needs: correct output across every input
of a bare ALU/data-movement sequence with no peripheral involvement.

Two things support trusting it for that narrower claim:

- The simulator decodes and executes real PIC18 opcodes directly (`match
  word & 0xFC00`/`0xFE00` on the actual instruction encoding), not a
  higher-level behavioral abstraction, with real flag computation
  (`add_flags`/`sub_flags`/`set_zn`) and real `BSR`-based bank addressing
  (`resolve_f`), not a simplified model.
- `crates/sim/tests/pic18_acceptance.rs` cross-checks the assembler's own
  output against `gpasm` and the simulator's execution against hand-worked
  expected RAM state, giving independent confidence in both halves this
  spike depends on.

One real caveat, stated rather than assumed away: `docs/05-verification.md`
notes gpsim, the independent semantic reference used to cross-check the
in-tree simulator, "supports the 14-bit core" only. The PIC18 `Pic18`
struct has no equivalent independent instruction-level cross-check the way
PIC14's `Pic14` does; its correctness rests on the acceptance test above
and on being exercised heavily elsewhere (every PIC18 e2e fixture in the
repo runs through it). This spike adds three of its own direct checks
before trusting anything downstream (`crates/superopt/src/lib.rs`,
`tests` module): `status_z_bit_reflects_a_real_addlw`,
`status_c_bit_reflects_a_real_addlw`, and
`btfss_skips_exactly_the_next_line`, each checking a real instruction's
effect against the physical STATUS address and the skip mechanism the
search alphabet leans on, rather than trusting the constants by
construction.

**Verdict: usable as an oracle for this spike's scope (bare ALU/data
instructions, no peripherals, no interrupts), with the gpsim-parity gap on
PIC18 as a known, not eliminated, residual risk.** A production
integration should not skip past this; see Recommendation.

## A soundness bug the review caught

The first draft's `Case` type carried only RAM pokes, and both target
tests looped over candidate entry-`W` values without any way to apply
them: `Pic18::new` hard-codes `w: 0`, so every run silently started with
`W == 0` regardless of what the test loop intended. Concretely, this made
`MOVWF dst` (store W to the destination) verify as correct in both
targets, because the only W value ever exercised was the one already
sitting in a fresh machine, not because the candidate was actually
independent of entry W. 3 of the 5 sequences the first draft reported were
false positives on that account, and the bool-materialization target's
headline word count was wrong as a result (see below).

Fixed by giving `Case` a real `entry_w: u8` field and `Pic18::set_w`
(`crates/sim/src/lib.rs`) to apply it before each run, plus two new tests
(`entry_w_reaches_the_machine`, and widening both targets' case sets to
vary entry `W` for real). The review also caught two smaller soundness
gaps in the same pass, both fixed: `run_case` accepted `Pic18::halted()`
on its own, but the simulator also reports `halted` when `pc` merely runs
off the end of `prog`, which happens for a candidate whose last
instruction is a taken skip that jumps past the appended `sleep` entirely;
`run_case` now additionally checks that `pc` lands exactly on the `sleep`
instruction's own address. And `shortest`'s panic-hook silencing (most
candidates in a brute-force sweep are expected to fail to assemble, which
is not worth a stack trace per attempt) now restores the previous hook via
a `Drop` guard, so a panic that somehow escapes `verify`'s own
`catch_unwind` cannot leave every later panic in the same process silent.

## Method

- A candidate is plain PIC18 asm text from a curated alphabet (not the full
  ISA; see each target's scoping note), assembled for real, run on a fresh
  `Pic18` per case with `SLEEP` appended so the run has a deterministic
  stop point, checked with entry `W` seeded via `Pic18::set_w`.
- No labels: the alphabet's conditional instructions are `BTFSC`/`BTFSS`,
  which skip exactly the next line on real hardware, so no branch-target
  resolution is needed to search straight-line sequences.
- Search is breadth-first by length (1, 2, 3, ... up to a cap), exhaustive
  within each length, over the candidate alphabet; the first length with
  any verified hit is reported in full (every candidate at that length, not
  just the first).
- A "verified" candidate matches every case in a curated case set, not
  every possible input: each target's test names what the case set
  covers. `run_case` (epic-cc#521) rejects any RAM byte outside a case's
  declared `allowed_changes` that changed from its post-poke value, so a
  candidate that clobbers an unrelated GPR no longer "verifies" by
  accident. `STATUS` and `W` are the two deliberate exceptions: both are
  treated as scratch, never part of either target's correctness contract,
  so a candidate is free to touch them. `run_case` also poisons all of RAM
  to a non-zero sentinel before applying a case's pokes, a fix from
  epic-cc#521's own review: without it, a clobbering write that happens to
  store zero to a byte no case ever pokes is invisible, since RAM already
  reads zero there by default.

## Target 1: bool-materialization (epic-cc#501)

Contract: after some prior comparison has already set `STATUS.Z`,
materialize `dst = (Z ? 1 : 0)`, store included. `isel-pic18`'s current
`BRA tmp0 / MOVLW 0x00 / BRA tmp2 / MOVLW 0x01` diamond is 4 words, and
the `MOVWF` that stores it (outside the diamond in the ticket's own count)
is a 5th; this spike's contract always ends with the value in RAM, so
every count here is "including the store", and **the isel-pic18 baseline
to beat is 5 words, not 4.**

Case set: every combination of Z, C, and entry W, the full 2 x 2 x 256 =
1024-case domain (epic-cc#521 widened this from a curated 12-case sample;
`no_unexpected_clobber`'s array-equality check keeps the full domain fast
enough, under 5s in a debug build, that a separate `--ignored` variant
turned out unnecessary), with the destination byte poisoned to `0x55`
before each run. C and W are dimensions a correct candidate must not
depend on; poisoning the destination catches a candidate that silently
leaves it untouched.

**Result: exhaustive search up to length 5 finds a floor of 4 words**, 9
candidates, e.g.:

```
movlw 0x01 / movwf 0x020,A / btfss 0xFD8,2,A / clrf 0x020,A
movlw 0x00 / btfsc 0xFD8,2,A / movlw 0x01 / movwf 0x020,A
```

One word better than isel-pic18's current 5, across 204 sites (per the
density profiler's count on `hal-pic18-menu-demo-18f4550`): roughly 200
words, not the larger number the first (buggy) draft of this document
reported.

**The ticket's own hand-derived 3-word candidate, `CLRF dst; B<inv> skip;
INCF dst,F`, does not verify, and should not be implemented as written.**
`CLRF` always sets `Z` (clearing anything to zero always produces zero),
so it destroys the incoming `Z` bit the very next instruction needs to
read, before that instruction ever reads it. Confirmed both by the search
(no `clrf`-first candidate verifies against a case set that varies
incoming Z independently) and by hand-tracing the sequence. This is a
correctness finding distinct from the density work: landing #501 by
implementing its own suggested fix literally would be a miscompile, not a
density win. Filed as a comment on epic-cc#501 directly.

This spike does not confirm or deny the ticket's other speculation, a
2-word carry-based sequence: that reshapes the *comparison* itself
(producing carry instead of just Z), a different, wider contract than the
one searched here.

## Target 2: shift-left-4 (epic-cc#505), scoped to 8-bit

epic-cc#505's repro is 16-bit (`x <<= 4` on `unsigned int`, 12 words: four
steps of `BCF STATUS,C` + `RLCF`/`RLCF`). This spike scopes down to a
single byte shifted in place, the natural first slice: the unrolled
baseline for one byte's worth of the same pattern is 4 steps of
`BCF`+`RLCF`, 8 words.

Case set: 5 low nibbles (0x0, 0x1, 0x7, 0x8, 0xF: zero, a lone low bit, a
lone high bit within the nibble, all ones) x 4 high nibbles (0x0, 0x3,
0x8, 0xF, checking the incoming high nibble never leaks into the shifted
result) x 3 entry-W values x 2 entry-C values, 120 cases. Not exhaustive
over all 256 byte values or all 256 W values; masks for nibble/byte-
boundary bit operations are naturally power-of-two shaped, so the
alphabet's literals (`0xF0`, `0x0F`) are curated the same way, and the
case set matches that scope.

**Result: exhaustive search up to length 4 finds a floor of 3 words, one
candidate, genuinely W- and C-independent:**

```
swapf 0x020,W,A / andlw 0xF0 / movwf 0x020,A
```

(Two other 3-word candidates the first draft of this document reported,
built on `IORLW 0xF0` after a `SWAPF ...,F`, turned out to depend on entry
W's low nibble already being clear; they no longer verify against the
corrected case set and are not real.)

`SWAPF` (nibble swap, one instruction, no flags) plus a mask beats the
unrolled rotate loop by 5 of 8 words (62.5%) for the single-byte case. This
is a real, actionable result for the byte-boundary special case, not yet a
claim about the general 16-bit shift-by-N contract #505 is actually
scoped to.

## Target 2, generalized to 16 bits and shift amounts 1-7 (epic-cc#520)

The 8-bit result above deliberately stopped short of #505's actual
contract. `crates/superopt/tests/shift_16bit.rs` covers it: two registers
(`LO`/`HI`, in place, matching how `x <<= n` compiles), shift amounts 1-7
(8 is already #470's byte-move case). `isel-pic18`'s baseline for any
amount N is N steps of `BCF STATUS,C` + `RLCF lo,F` + `RLCF hi,F`, 3
words/step, so **the baseline to beat is 3*N words for every amount**.

Exhaustive search over this contract does not stay tractable at the
baseline's own length: the alphabet needed (rotate ops in both
directions plus the nibble-swap family, ~28 symbols) makes `28^9`
(amount 4's baseline length) far beyond a bounded-effort search, and even
a length-4 bound at that alphabet size is too slow for the default test
suite (measured: 2.5 minutes for the file, against 5s at length 3). Two
methods instead of one:

- **Bounded exhaustive search**, up to length 3, over the combined
  alphabet. Amount 1's 3-word baseline is inside this bound, so a
  confirmed floor there is a real answer; amounts 2-7 find nothing this
  short, the honest result of the bound, not evidence the baseline is
  optimal.
- **A constructed candidate, verified with the same `verify()` the
  search engine uses**, not hand-traced, checked against the full
  65536-input domain (not a curated sample, since a construction is one
  fixed candidate rather than a combinatorial search): for amount 4, the
  8-bit nibble-swap trick generalized across both bytes (`SWAPF` both,
  mask, recombine the nibble that straddles the byte boundary). This is
  one instance of a general family, rotate each byte left by `n`, mask,
  recombine the bits that straddle the byte boundary, costing
  `2 * rotate_cost(n) + 7` words; `SWAPF` makes `rotate_cost(4) == 1`.

**A first draft of this section significantly under-found amounts 6 and
7, and misstated why.** The first draft's alphabet had no right-rotate
instructions (`RRCF`/`RRNCF`) at all, so it could not express the family
for any `n` other than 4, at any search length, and the constructed
candidates for 5-7 were the amount-4 construction with more standard
rotate steps appended rather than the family's own instance at the
right `n`. `RRNCF` makes `rotate_cost(6) == 2` (two single-bit right
rotates equal one left rotate by 6), giving an 11-word amount-6
construction against the 15 the first draft reported. Amount 7 does
better still with a different trick (not the family): a 16-bit logical
right-shift-by-1 (`BCF C` then `RRCF hi,F` then `RRCF lo,F`, carry seeded
0) turns `x << 7` into a byte move plus one more rotate, 7 words against
the first draft's 18.

**Results:**

| amount | baseline | bounded search (<=3) | constructed | delta |
|---|---|---|---|---|
| 1 | 3 | floor 3 (confirmed minimal, this alphabet, this bound) | -- | 0 |
| 2 | 6 | nothing found | -- | unknown |
| 3 | 9 | nothing found | -- | unknown |
| 4 | 12 | nothing found | **9** | **-3 (25%)** |
| 5 | 15 | nothing found | **12** (family instance is 13, worse) | **-3 (20%)** |
| 6 | 18 | nothing found | **11** | **-7 (38.9%)** |
| 7 | 21 | nothing found | **7** | **-14 (66.7%)** |

Amounts 2 and 3 have no result either way: the family construction
exists at every `n` (cost `2 * rotate_cost(n) + 7`), it simply costs more
than the `3*n` baseline for `n <= 3`, so nothing was constructed for
them, not because no shortcut exists off the nibble boundary (the first
draft's stated reason here was wrong) but because the shortcut that does
exist loses at these amounts specifically.

**Every constructed candidate above clobbers `W`**, unlike the baseline
(`BCF`+`RLCF`+`RLCF` never touches it). `run_case`'s clobber check does
not catch this: `W` is excepted from it by design, the same as `STATUS`
(see the Method section above). Whoever lands one of these in
`isel-pic18` needs to confirm `W` is dead at the call site first.

## Recommendation

**Worth a follow-up integration ticket, scoped narrowly.** Target 2 in
particular (`SWAPF`+mask for a nibble-aligned shift) is a concrete,
unclaimed codegen opportunity distinct from what #505 already proposed
(which only covers shift-by-a-multiple-of-8 today, per #470). Target 1's
win is smaller than first reported (1 word/site, not more) but still real,
and more importantly the search disproved the ticket's own proposed fix
before it could ship as a correctness bug. Recommended scope for that
follow-up, not attempted here:

- ~~Generalize target 2 to the 16-bit case #505 actually reports
  against, and to shift amounts other than 4~~: done, epic-cc#520 (see the
  results table above). Amounts 4-7 each save a real, verified 3 words;
  amounts 2 and 3 got no answer either way within a tractable search
  bound, and no comparable algebraic shortcut was found for them.
- ~~Widen target 1's case set~~ and ~~widen the verification check to the
  whole machine state~~: done, epic-cc#521. Both surfaced real soundness
  gaps of their own on the way (a `W` register that was never actually
  seeded, then a RAM-clobber check blind to writes that happen to store
  zero), the same category of bug this spike's own #514 review found once
  already. Worth naming as a pattern: this crate has now found a real
  soundness bug in itself on both rounds it has been reviewed.
- Decide, before wiring anything into `isel-pic18`: does closing the
  gpsim-parity gap on PIC18 (an independent semantic cross-check, not just
  the acceptance test and this spike's three direct checks) become a
  prerequisite once a superoptimizer result is trusted enough to change
  shipped codegen, rather than just to confirm or disprove a human's
  hand-derived candidate.

**Not recommended for this spike's stretch goals yet:** an LLM in the
search loop, or an SMT-based prover in place of exhaustive-input
simulation. Both alphabets here were small enough that brute force finished
well within a normal test run; reach for either only once a target
primitive's input or instruction-alphabet domain is too large to enumerate
directly, and say so explicitly when that happens rather than reaching for
either as a default.
