# 45: Copy-coalescing spike findings (epic-cc#664)

Spike, not a production commitment: price giving a value and its copy
the same slot (copy coalescing, the standard register-allocation
technique applied to our static frames) on the menu-demo listing, with
and without code factoring (#662), and turn the result into a verdict
on the follow-up. Everything below comes from our own listing (ADR-006).

Baseline: the `--emit asm` listing (`make size-report`,
`scratch/size-report/menu-demo.asm`, 8828 lines) carries 1166 `MOVFF`
instructions, 2332 words, about 24% of flash.

## Method

Three stages, all in `scratch/664/` (analyzers stay scratch per the
ticket; nothing below ships except this doc).

**Classify every `MOVFF`.** One syntactic pass over each listing, first
match wins: FSR setup (a side is `FSR0L/H` or `FSR1L/H`; 34 with
factoring against 30 hand-counted, the gap is single `MOVFF` loads of
`FSR` inside repeated sequences the pass then shares), SFR or
indirect (a side at or above `0xF60`, or an `INDF`/`POSTINC` register),
return value (a side in the `0x000-0x00F` retval reservation), argument
setup (within 8 lines before a call, heuristic, see Residuals), then
block copy (address-adjacent run with the neighbour) or slot-to-slot.
Function ownership comes from entry labels (EQU `func.var` prefixes,
entry points, long-`CALL` targets); labels reached from another owner
are shared (outlined) bodies.

**Static liveness rule.** A copy `MOVFF s,d` in one function can share
a slot when: both sides are that function's frame slots (one owner,
non-ISR, address never taken as an `LFSR` literal, no touch inside a
shared body); nothing reads `s` before `s` is next redefined; nothing
reads `d` before `d` is next redefined; and `s` stays untouched until
`d`'s last read. Backward branches get a guard: inside a loop region
both sides must be otherwise untouched, because a static-before access
can execute dynamic-after across a back edge. Each guard exists because
the simulator caught its absence (see Behaviour check).

**Padded twin differential.** Each surviving copy becomes two `NOP`s
(one `MOVFF` is 2 words and 2 cycles, so the twin is layout- and
cycle-identical) and later reads of the merged slot are renamed to the
survivor; this sample needed no renames, every proven copy is
dead-at-copy. Both images assemble through `assemble_pic18` and run in
the PIC18 simulator comparing the ordered RAM-write stream outside the
merged slots: exact to the harness halt with no interrupts, and prefix
runs with interrupts every 97 and 13 writes (the `writes()` shape from
the outlining gate). Disagreements are bisected to single copies.

## Results

| class | copies | words |
|---|---|---|
| argument setup (8-line window heuristic) | 241 | 482 |
| return value | 101 | 202 |
| slot-to-slot | 159 | 318 |
| block copy (address-adjacent runs) | 346 | 692 |
| FSR setup | 34 | 68 |
| SFR or indirect access | 285 | 570 |
| total | 1166 | 2332 |

813 of the 1166 sit inside shared (outlined) bodies and are out of
listing-level reach: a merge there must hold in every caller context,
which the listing alone cannot show.

Without factoring (`--no-outline`, same sources) the listing carries
1553 `MOVFF`s, 3106 words: argument setup 208, return value 137,
slot-to-slot 238, block copy 486, FSR setup 104, SFR or indirect 380.
The static rule proposes the same init cluster there (11 candidates, 9
of the same slot pairs), so the init copies are unique code in both
configurations while the repeated shapes account for the 387-copy gap.
The sim proof ran on the factored listing only, which is the shipped
configuration. Classifier transcripts for both runs are kept as
`scratch/664/classify-factored.log` and `classify-nooutline.log`.

Of the own-function slot and block copies (157 in scope), the loop
guard leaves 11, and sim bisection proves 6 dead (12 words), all in
`menu_demo_init` except one in `main`:

| listing line (0-based script index into the factored listing) | copy | sim |
|---|---|---|
| L4343 | `MOVFF 0x02C, 0x25C` | dead |
| L4345 | `MOVFF 0x02C, 0x25D` | dead |
| L4240 | `MOVFF 0x047, 0x05F` | dead |
| L4466 | `MOVFF 0x074, 0x023` | dead |
| L4527 | `MOVFF 0x06E, 0x023` | dead |
| L6926 | `MOVFF 0x2BF, 0x017` | dead, in `main` |
| L4467 | `MOVFF 0x075, 0x028` | live |
| L4241 | `MOVFF 0x044, 0x060` | live |
| L4479 | `MOVFF 0x071, 0x022` | live |
| L4526 | `MOVFF 0x06D, 0x022` | live |
| L4628 | `MOVFF 0x07F, 0x022` | live |

The composed twin (all 6 dead copies padded at once) matches the
original exactly: same halt, same streams, same interrupt runs.

Two of the live ones show the channels a listing analysis cannot see.
`0x060` is named exactly once in the whole listing, at its copy, yet
removing the copy diverges: it is read through `FSR1` seeded two bytes
below (`LFSR 1, 0x05E` plus an index), so no literal ever names it.
`0x022` is scratched by a dozen functions under the overlay and read
inside a shared body, which first looked like single ownership until
the multi-caller check counted correctly. The remaining three live
copies diverge the same way with their channels unattributed.

RAM: removing the 6 dead copies saves 12 words of flash and
approximately nothing of RAM. The dst slots stay allocated to
neighbouring values, so no frame shrinks. Genuine coalescing (merging
two live ranges into one slot) could shrink frame peaks and through
them the overlay, but that needs the allocator, so RAM stays unpriced
here.

## Behaviour check

The differential caught two analyzer bugs before any number was
trusted. First, a loop-carried copy the linear rule accepted: the dst
is tested at the loop top before the copy rewrites it, so the copy
feeds the next iteration. The twin never halted. Second, the
shared-context illusion above: discarding shared-body touches made a
scratch slot look frame-private. The twin diverged. Both guards above
are the fixes, and every survivor since passed alone and composed.

Coverage limit: the runs follow the harness path to its halt plus two
interrupt schedules. Copies dead on these paths could be live on
unexecuted ones (error branches, untested menu routes). The 12-word
price is therefore a floor on executed paths, not a whole-program
proof. An implementation in the allocator reasons from IR liveness and
does not carry this limit.

## Verdict: reshape

Do not build listing-level copy coalescing. Twelve proven words
(0.5% of the `MOVFF` bucket) after this much analysis says the listing
is the wrong place: outlining shares the hot slots across contexts,
and the channels that matter (indexed `FSR` congeners, shared bodies,
loop-carried values) are invisible there by construction.

The direction that survives is coalescing in `alloc`, where liveness
is exact: when assigning frame slots, give an IR copy (phi edge,
byval argument, return-value store) its source's slot whenever their
live ranges are disjoint, and teach `isel-pic18` to skip a copy whose
sides resolved to the same slot. That also prices the classes this
spike could not: the 241 call-argument copies (call-aware slot reuse),
the 101 return-value copies (compute into the retval region), the 346
block copies (overlap-checked range merges), and the shared-body
copies (which stay shared but shrink at every site at once).

Recommended follow-ups, in price order:

- **alloc slot coalescing** for IR copies with disjoint live ranges,
  with the bisection twin as its differential gate.
- **isel-pic18 self-copy skip**: drop `MOVFF x, x` after coalescing
  (today nothing ever produces one, so this is the two-line companion
  of the alloc change, not a standalone win).
- **Reprice after landing**: rerun the classifier from the rules in
  Method against the new listing. The 12 proven words should fall out
  without special effort, and the arg/retval/block classes get their
  first exact numbers.

## Residuals

- The analyzer and twin harness are scratch (`scratch/664/`, ignored
  by git) and did not ship. Reproduction: `make size-report`, then
  `python3 scratch/664/classify.py` and `rewrite.py`, then the twin
  test shape from the outlining gate pointed at the two listings.
- The twin test (`scratch_664_twin.rs`) was temporary and is deleted;
  only this doc ships.
- The 8-line argument window over-counts (241 vs the ticket's 94).
  Exact argument attribution needs call-aware slot knowledge, which is
  the alloc ticket's first input, not this spike's.
- The true relationship between the stages: the static rule proposes
  11 candidates, the simulator confirms 6 of them dead. A stricter
  variant (any shared-body touch disqualifies) proves 0, which only
  restates the reshape verdict: listing-level proof cannot close the
  gap, simulation and then alloc-side liveness must.
- PIC14 and PIC14E are unmeasured. Their listings have no outlining
  and no `MULWF`, so both the classification and the price differ.
