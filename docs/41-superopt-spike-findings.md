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

`Pic18` is already used as a behavioral gate elsewhere (sim-verified e2e
fixtures, the gpasm HEX cross-check, hand-worked expected state), but that
is a different, weaker claim than what this spike needs: correct output
across every input of a bare ALU/data-movement sequence with no peripheral
involvement. (This document previously cited "XC8-differential whole-program
runs" as one of those gates; no such runs exist, for either core. See
`docs/05-verification.md` and epic-cc#527.)

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

**Postscript, epic-cc#518: the diamond this section measured against is
gone from the common compare-and-store shape.** `isel-pic18` now preclears
the result slot before the compare (`bool_result_preclear`) and lets the
compare's own branches select a single `INCF` on the true arm: 2 words of
materialization, no diamond, no trailing `MOVWF`. That beats this spike's
4-word floor, and it can only be seen by a compiler, not by this spike's
straight-line contract: the win comes from emitting the clear before the
flag-setting compare, a reordering no fixed candidate sequence can
express. The shipped lowering also sidesteps the `CLRF`-first hazard
above purely by ordering, so the ticket's candidate was never implemented
as written.

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

| amount | baseline | bounded search (<=3) | deep search (<=5) | constructed | delta |
|---|---|---|---|---|---|
| 1 | 3 | floor 3 (confirmed minimal, this alphabet, this bound) | -- | -- | 0 |
| 2 | 6 | nothing found | **nothing found (exhaustive, epic-cc#528)** | -- | 0 (baseline stands) |
| 3 | 9 | nothing found | **nothing found (exhaustive, epic-cc#528)** | -- | 0 (baseline stands) |
| 4 | 12 | nothing found | -- | **9** | **-3 (25%)** |
| 5 | 15 | nothing found | -- | **12** (family instance is 13, worse) | **-3 (20%)** |
| 6 | 18 | nothing found | -- | **11** | **-7 (38.9%)** |
| 7 | 21 | nothing found | -- | **7** | **-14 (66.7%)** |

Amounts 2 and 3 are now answered exhaustively (epic-cc#528): the deep
run enumerated every candidate of lengths 4 and 5 over this alphabet,
17,825,024 per amount, and found **none**. Combined with length 3 from
the bounded search, no sequence of 5 instructions or fewer computes
either shift, so the `3*n` baseline (6 words at amount 2, 9 at amount 3)
is minimal at that bound. Cost was 852.8s and 856.1s, matching the
48.3us/candidate rate measured beforehand (a no-hit run pays the full
4+5 sweep: 614,656 + 17,210,368 candidates). The family construction
exists at every `n` (cost `2 * rotate_cost(n) + 7`) but costs more than
baseline for `n <= 3`, which is why nothing was constructed for them.

This closes the "no answer either way" the table previously recorded, and
retires the question: a shorter form would have to be at least 6
instructions, longer than the baseline it would replace at these amounts.
The deep run is `#[ignore]`d in `crates/superopt/tests/shift_16bit.rs`
(`deep_search_amounts_2_and_3`), so it costs the default suite nothing.

**Every constructed candidate above clobbers `W`**, unlike the baseline
(`BCF`+`RLCF`+`RLCF` never touches it). `run_case`'s clobber check does
not catch this: `W` is excepted from it by design, the same as `STATUS`
(see the Method section above). Whoever lands one of these in
`isel-pic18` needs to confirm `W` is dead at the call site first.
(Confirmed at the epic-cc#505 landing: no `isel-pic18` lowering reads `W`
before writing it within the same statement, there is no cross-statement
W tracking (#502 is the ticket to add one), and WREG sits inside the ISR
save area, so an interrupt cannot lose it either.)

## Target 3: right shifts, and the 32-bit generalisation (epic-cc#526)

epic-cc#505 landed the left-shift forms; everything else still unrolled per
bit, because nothing better was verified. #526 is the request to close that
gap with the same discipline: construct, verify, then wire. Three results.

**Right shifts, single lane, amount 4 (any width): landed.** The mirror of
the landed left form. A left shift by 4 is `SWAPF lane,W ; ANDLW 0xF0 ;
MOVWF lane` (keep the high nibble, move it up); a right shift by 4 is
`SWAPF lane,W ; ANDLW 0x0F ; MOVWF lane` (keep the low nibble, which after
the swap is the source's high nibble, now in the low position). Three
words against the unroll's `4 * (BCF + RRCF)` = 8 for a byte and 12 for a
16-bit pair. Verified over the full 256-input byte domain and independent
of entry `W` and `C` (`crates/superopt/tests/shift_right_4.rs`), and the
landed `isel-pic18` form is re-checked by simulating the real selector's
output over the same domain. W-only, so it shares the landed left forms'
dead-`W` precondition.

**Right shifts, 16-bit pair, amount 4: landed, 9 words.** The two-lane
mirror is not a composition of the single-lane form (bits carry across the
byte boundary), but it is still W-only:

```
lo' = (lo >> 4) | ((hi & 0x0F) << 4)      hi' = hi >> 4
```

Nine words against the 12-word unroll, verified over the full 65536-input
domain plus a `W` sample (`crates/superopt/tests/shift_right_16bit.rs`).
Both forms fire on any width's single surviving lane at `r == 4`, including
`m > 0` cases (`lshr i16, 12`, `lshr i32, 28`), so the landed selector is
simulated over the whole domain for each of those too
(`crates/isel-pic18/tests/isel_pic18.rs`).
Getting this far caught a real trap: a first draft had a malformed
operand (`movf 0x022,A`, no `F`/`W`), which the assembler rejects by
panicking, and `verify()` treats a panicking candidate as *not verified*.
A malformed line and a wrong derivation look identical from the outside,
so the fix was to check which branch the test actually took rather than
trusting its green.

**Measured on the epic's own fixture: -59 flash words.** The menu-demo
ladder entry drops from 11890 to 11831 words after these forms land
(`crates/driver/tests/fixtures/size_baseline.toml`). Attribution is
exact, not inferred: with the right-shift fast path compiled out the same
tree measures 11890, master's number to the word, and with it in, 11831.
The fixture's right shifts by 4 (`epic_lcd_gpio4.c`'s
`(uint8_t)(byte >> 4u)`, `sim_menu_demo.c`'s `(v >> 4) & 0xFU`) are the
whole delta.

**32-bit shifts by 4, both directions: verified, and they lose.** The
natural in-place, W-only 4-lane generalisation of the 16-bit family is 21
words at amount 4 (`3 * 6 + 3`: each lane above the lowest becomes
`(lane << 4) | (lower >> 4)` in 6 words, the lowest `<< 4` in 3). The
unroll it would replace is `r * 5` words (`r` steps of one `BCF` plus one
rotate per live lane), 20 at amount 4. So the construction **loses by one
word** at amount 4.

The tempting reading is that the same 21-word form would win at amounts
5-7, where the unroll is 25/30/35. That reading is wrong, and checking it
is why amount 5 is also built and verified here: a shift by 5 needs the
nibble form *plus* a fifth single-bit pass, 5 more words, so the
construction is `21 + 5*(r-4)` = `5r + 1` against the unroll's `5r`. It
loses by exactly one word at every amount, not only at amount 4
(`construction_shl5`, verified over the same sample). The asymmetry with
the 16-bit case is the point: there the construction grows 6 words per
lane while the baseline grows only 2 per step, so the 16-bit forms win at
every amount; here the construction's per-step cost (5 words, the extra
bit pass) matches the unroll's exactly, so the amount-4 nibble saving is
consumed by the time the fifth bit is counted. A form that fuses the extra
bits into the nibble pass, the way the 16-bit target found separate
constructions for amounts 6 and 7, was not searched; if one exists it
would be the first 32-bit win, and the amounts 5-7 unrolls are where it
would pay.

The 32-bit constructions are verified over a curated 4-byte sample
(`crates/superopt/tests/shift_32bit.rs`), not the full 2^32 domain: a
full-domain run at this crate's per-case cost is days, and the ticket
names the curated-sample treatment for this width. They are **not wired
into `isel-pic18`**, because a form that costs more than the code it
replaces is not worth landing; `const_shl_i32_by_4_keeps_the_unrolled_form`
still pins the unroll, and a companion test records that the 4-lane form
is correct and one word too long, so the negative is checked rather than
asserted. The search bound at this width is hopeless
(`28^10` candidates, ~4 million hours on the measured 48.3us/candidate
rate), so no exhaustive answer is claimed for 4 lanes: the result is
"this construction loses", not "nothing shorter exists".

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
  results table above). Amounts 4-7 each beat the baseline by a real,
  verified margin (3 to 14 words, per the table);
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
- ~~Whether any of these verified sequences are worth landing in
  `isel-pic18`~~: landed for the shift targets, epic-cc#505. The
  single-lane `SWAPF`+mask form covers any width's 4-bit residual, and
  the 16-bit amount 4-7 constructions emit directly; every landed form is
  re-verified in `crates/isel-pic18`'s own tests by simulating the real
  selector's output over the full input domain (65536 values per 16-bit
  amount, 256 per byte lane), not by trusting the superopt suite's
  candidate text.
- ~~Right shifts, all widths, and the 32-bit generalisation~~: done,
  epic-cc#526 (Target 3 above). Single-lane and 16-bit right shifts by 4
  are verified and landed (3 and 9 words against 8 and 12); the 32-bit
  left and right 4-lane forms are verified *and lose* (21 against 20), so
  they are deliberately not landed. The general lesson recorded there:
  the 16-bit family wins because it grows 6 words per lane against a
  baseline growing 2 per step, and that advantage inverts at 4 lanes
  (baseline 5 per step). Target 1 needed no landing: the #518 postscript above
  records the common shape already beating its floor. #505's own second
  lowering, a counted loop above a word-count threshold, was rejected on
  arithmetic: `DECFSZ`+`BRA` costs 2 more words per iteration than the
  unrolled step it replaces, plus setup, so no width or amount reaches a
  break-even point.
- The gpsim-parity question is resolved for this landing, not in general:
  the shift sequences shipped without an independent PIC18 oracle, on the
  argument that a fixed candidate checked by full-domain simulation
  carries a much smaller risk class than sampled-oracle codegen, and that
  the gap weighs on every PIC18 lowering equally, not specially on
  superoptimizer-sourced ones. Whether to close the gap anyway stays open
  as its own ticket.

**Not recommended for this spike's stretch goals yet:** an LLM in the
search loop, or an SMT-based prover in place of exhaustive-input
simulation. Both alphabets here were small enough that brute force finished
well within a normal test run; reach for either only once a target
primitive's input or instruction-alphabet domain is too large to enumerate
directly, and say so explicitly when that happens rather than reaching for
either as a default.
