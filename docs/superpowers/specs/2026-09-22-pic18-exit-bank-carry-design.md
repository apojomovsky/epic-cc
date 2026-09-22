# PIC18 interprocedural BSR exit-bank carry (design)

Date: 2026-09-22. Scope: epic-cc#495, approach A gated by C, approved in
brainstorming. Goal: remove the MOVLBs that survive on PIC18 only because
the tracked BSR state is discarded at every call return, without paying
one word per return site and without growing RAM.

## Status

The restore-on-return convention sketched in #495 is rejected on
evidence: `docs/42-density-ceiling-findings.md` (Experiment 1) measured
it at fewer net words than the join tracking that has since landed with
#534, because 63 return sites each pay a restore word. The outcome this
ticket serves, fewer menu-demo MOVLBs, is pursued instead by carrying
provable callee exit banks into the caller, which adds no instruction
anywhere.

## Problem

After #534, `isel-pic18` keeps BSR state across branch joins that agree
and poisons it where they do not. The remaining clears are structural:
function entry assumes the caller leaves any bank, a direct call clears
the tracked bank because the callee never restores a known bank on
return, an indirect call clears after every candidate, and labels whose
predecessor end states disagree or include a back edge stay unknown.
The first two are the target: the whole program is compiled in one
process, so what the callee's return paths do with BSR is knowable.

## Mechanism

All changes live in `crates/isel-pic18`. There is no second analysis
that could drift from the emitter: the emitter's own per-function `Gen`
run is the analysis.

1. The call graph is derived from the module's direct call instructions.
   Recursion is already a compile error upstream, so the graph is a DAG.
2. Functions are emitted into per-function buffers in reverse
   topological order, callees first. Buffers are concatenated in the
   original module order, so the `asm` stage boundary sees byte
   identical text apart from elided MOVLBs.
3. After a function's buffer is complete, its exit bank is the join
   (meet) over the already recorded end states of its RETURN-ending
   blocks. All recorded ends equal and present gives a known bank; a
   dirty end, a missing state, or a disagreement gives unknown.
   The result lands in an exit-bank map keyed by function name.
4. The direct call arm and each indirect call candidate set the tracked
   bank to the callee's map entry when known, and clear it when not,
   which is exactly today's behavior. An indirect call takes the meet
   over all candidates: any unknown candidate clears.
5. Runtime recipes and ISR-suffixed copies are not IR driven, so they
   are absent from the map and callers treat them as unknown. No call
   site is ever less conservative than master.

Nothing else changes. No instruction is added at any return site, the
entry-block assumption stays "callers leave any bank", alloc and the
access window are untouched, and `bsr_dirty`, block join agreement, and
forward label joins keep their current meaning.

## Soundness

The exit-bank map is derived from the end states the same run recorded
for the code as actually emitted, so it cannot disagree with the
emitted text. Meet over returns means every return path of the callee
ends at the carried bank before a caller relies on it. The simulator
remains the behavioral oracle for every elided MOVLB.

## Gate

Before implementing, profile the menu-demo on master with
`scripts/density-profile.py` and count the MOVLBs that sit at the first
banked access after a call whose callee has a provable exit. If that
population is negligible, the ticket closes with the numbers and the
findings filed, and no code lands. The numbers are reported either way
before proceeding.

## Acceptance

- Every provable-exit, call-adjacent MOVLB on the menu demo is gone,
  with before and after counts reported on the ticket.
- No flash growth anywhere in the size ladder: the change only elides
  MOVLBs, so rows shrink or hold; the menu-demo row is re-baselined
  deliberately with `UPDATE_SIZE_BASELINE=1`.
- The full suite is green in the container; the simulator verifies
  every elided MOVLB.
- The decision is distilled into an ADR (carry mechanism, convention
  rejected per docs/42) with an index line in `docs/03`; spec and plan
  files are removed in the final commit per the takeoff ritual.

## Testing

Unit tests in `crates/isel-pic18/tests/isel_pic18.rs`:

- a caller skips the re-bank after a call whose callee has a provable
  exit bank;
- a callee with a dirty exit, returns that disagree, or an absent map
  entry keeps today's clear;
- an indirect call carries only when every candidate agrees;
- a diamond whose arms call the same callee still carries;
- recipe-called sites are unchanged.

The existing call-return-invalidates test gains a sibling whose callee
has an unknown exit; it is narrowed, not deleted. One e2e fixture with
a call-adjacent banked access runs through the full pipeline and the
simulator. One test pins the emission-order invariant: a module whose
reverse topological order differs from its module order assembles in
original order.

## Non-goals

- The restore-on-return convention: rejected, see Status.
- Exit banks for runtime recipes: filed as a follow-up if the profile
  shows float-heavy call sites matter.
- Back-edge loop-header unknowns: a linear isel pass cannot see them;
  if the re-profile shows residual, that is a filed follow-up in
  banking-pass territory, not scope here.
- A PIC18 port of the `banking` crate: rejected for now; it duplicates
  the tracking isel already does and the driver deliberately bypasses
  banking for PIC18.
