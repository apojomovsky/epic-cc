# ADR-052 -- All-constant search loops fold by guarded unrolling

**Status:** Accepted 2026-10-12<br>
**Decides:** `epic-cc#1212`<br>
**Evidence:** `crates/driver/tests/const_period_search_e2e.rs` (folded
asm plus sim result, runtime loop beside a search stays rolled),
encoder-sim size row measured base and head in one run: 6495 to
5215 flash words, 262 to 237 RAM bytes

## Context

`epic_tick_init` ran a 3x16x2 prescaler search with u32 divide at
every boot, although its only caller passes a constant clock. The
base whole-program list specializes the argument (`ipsccp`) but
cannot evaluate the loop: clang `-O1` stamps every loop with
`llvm.loop.unroll.disable`, which LLVM's unroller honours
unconditionally, and the default unroll threshold stops short of
the search body anyway.

## Decision

- After the base list, at every profile except `-O0`, a second
  `opt` run attempts the fold, gated on the metadata edit below
  actually changing the base text.
- The edit removes the unroll-disable mark from loops in the
  nest of a constant divide: a loop whose natural loop (header
  plus blocks reaching the latch) holds a divide or remainder
  with a constant operand, and loops nested inside it. It then
  unrolls at a raised threshold and cleans up (`sccp`/`adce`
  collapse the constant chain the unroll leaves).
- The folded text ships only when a marked function shrinks in
  instruction count and no function grows. Marked functions that
  do not shrink are dropped and the fold reruns without them, so
  their loops are never unrolled in the shipped text. A failed
  `opt` run likewise keeps the base output.

## Rationale

The per-loop edit keeps sibling runtime loops rolled, and the
per-function guard stops one function's savings from subsidizing
another's growth, so the stage needs no per-program tuning or
baseline exception. `-O0` skips the stage because its contract
leaves control flow untouched for stepping and bisection.

Known limitation: an explicit `#pragma clang loop unroll(disable)`
emits the same metadata node as the automatic `-O1` stamp, so a
pragma-disabled loop that itself holds a constant divide is
still eligible. The guard bounds its size either way.

## Rejected alternatives

- Adding `loop-unroll` to the base list: the disable marks make
  it a no-op on `-O1` IR, and without the guard a raised
  threshold unrolls runtime loops into flash regressions.
- Replacing the marks with `llvm.loop.unroll.full`: forces the
  loop to expand fully, whatever its trip count, where the
  threshold plus guard already fold the constant case.
- A module-total guard: one function's fold savings can mask
  another function's unrolled growth, so the comparison is per
  function, with target functions required to shrink.
