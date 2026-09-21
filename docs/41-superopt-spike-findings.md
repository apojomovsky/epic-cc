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

## Method

- A candidate is plain PIC18 asm text from a curated alphabet (not the full
  ISA; see each target's scoping note), assembled for real, run on a fresh
  `Pic18` per case with `SLEEP` appended so the run has a deterministic
  stop point.
- No labels: the alphabet's conditional instructions are `BTFSC`/`BTFSS`,
  which skip exactly the next line on real hardware, so no branch-target
  resolution is needed to search straight-line sequences.
- Search is breadth-first by length (1, 2, 3, ... up to a cap), exhaustive
  within each length, over the candidate alphabet; the first length with
  any verified hit is reported in full (every candidate at that length, not
  just the first).
- A "verified" candidate matches every case in a curated case set, not
  every possible input: each target's test names exactly what the case set
  covers and what it deliberately does not (see below).

## Target 1: bool-materialization (epic-cc#501)

Contract: after some prior comparison has already set `STATUS.Z`,
materialize `dst = (Z ? 1 : 0)`. `isel-pic18` currently emits a 4-word
branch diamond (`BRA`/`MOVLW`/`BRA`/`MOVLW`) for this, the largest single
sink the density profiler named besides struct copies (816 words, 204
sites). The ticket names two hand-derived candidates to check: `CLRF dst;
B<inv> skip; INCF dst,F` (3 words), and speculates a 2-word carry-based
sequence might exist.

Case set: every combination of Z, C, and W (`false`/`true` x
`false`/`true` x three W values), with the destination byte poisoned to
`0x55` before each run, 12 cases. C and W are dimensions a correct
candidate must not depend on; poisoning the destination catches a
candidate that silently leaves it untouched.

**Result: exhaustive search up to length 4 finds a floor of 3 words**, two
candidates:

```
btfsc 0xFD8,2,A / movlw 0x01 / movwf 0x020,A
movwf 0x020,A   / btfsc 0xFD8,2,A / incf 0x020,F,A
```

This matches the ticket's hand-derived candidate and, within this
alphabet and case set, rules out anything shorter: no 1- or 2-word
sequence verifies. It does **not** confirm or deny the ticket's 2-word
carry-based speculation, since that reshapes the *comparison* itself
(producing carry instead of just Z), a different, wider contract than the
one searched here.

## Target 2: shift-left-4 (epic-cc#505), scoped to 8-bit

epic-cc#505's repro is 16-bit (`x <<= 4` on `unsigned int`, 12 words: four
`BCF STATUS,C` + two `RLCF` steps). This spike scopes down to a single
byte shifted in place, the natural first slice: the unrolled baseline for
one byte's worth of the same pattern is 4 steps of `BCF`+`RLCF`, 8 words.

Case set: 20 cases crossing a curated set of low nibbles (0x0, 0x1, 0x7,
0x8, 0xF: zero, a lone low bit, a lone high bit within the nibble, all
ones) against a curated set of high nibbles (0x0, 0x3, 0x8, 0xF), checking
that the incoming high nibble never leaks into the shifted-out result.
This is deliberately not exhaustive over all 256 byte values; masks for
nibble/byte-boundary bit operations are naturally power-of-two shaped, so
the alphabet's literals (`0xF0`, `0x0F`) are curated the same way, and the
case set matches that scope.

**Result: exhaustive search up to length 4 finds a floor of 3 words**,
three candidates, all built on `SWAPF`:

```
swapf 0x020,W,A / andlw 0xF0 / movwf 0x020,A
iorlw 0xF0 / swapf 0x020,F,A / andwf 0x020,F,A
swapf 0x020,F,A / iorlw 0xF0 / andwf 0x020,F,A
```

`SWAPF` (nibble swap, one instruction, no flags) plus a mask beats the
unrolled rotate loop by 5 of 8 words (62.5%) for the single-byte case. This
is a real, actionable result for the byte-boundary special case, not yet a
claim about the general 16-bit shift-by-N contract #505 is actually
scoped to.

## Recommendation

**Worth a follow-up integration ticket, scoped narrowly.** Both targets
found real, verified wins within budget, and target 2 in particular
(`SWAPF`+mask for a nibble-aligned shift) is a concrete, unclaimed codegen
opportunity distinct from what #505 already proposed (which only covers
shift-by-a-multiple-of-8 today, per #470). Recommended scope for that
follow-up, not attempted here:

- Generalize target 2 to the 16-bit case #505 actually reports against,
  and to shift amounts other than 4 (shift-by-4-mod-8 is the case
  `SWAPF` wins outright; other amounts likely need a different, possibly
  worse, trade).
- Widen target 1's case set from curated W/C values to the full 256 x 2 x
  2 domain now that the search itself is proven fast (the current run
  finishes in well under a second; full coverage is cheap).
- Decide, before wiring anything into `isel-pic18`: does closing the
  gpsim-parity gap on PIC18 (an independent semantic cross-check, not just
  the acceptance test and this spike's three direct checks) become a
  prerequisite once a superoptimizer result is trusted enough to change
  shipped codegen, rather than just to confirm a human's hand-derived
  candidate.

**Not recommended for this spike's stretch goals yet:** an LLM in the
search loop, or an SMT-based prover in place of exhaustive-input
simulation. Both alphabets here were small enough that brute force finished
before either would have mattered; reach for them only once a target
primitive's input or instruction-alphabet domain is too large to enumerate
directly, and say so explicitly when that happens rather than reaching for
either as a default.
