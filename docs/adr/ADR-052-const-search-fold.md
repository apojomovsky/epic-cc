# ADR-052 -- All-constant search loops fold by guarded unrolling

**Status:** Accepted 2026-10-12<br>
**Decides:** `epic-cc#1212`<br>
**Evidence:** `crates/driver/tests/const_period_search_e2e.rs` (folded
asm plus sim result), encoder-sim size row measured base and head in
one run: 6495 to 5215 flash words, 262 to 237 RAM bytes

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
  `opt` run attempts the fold when the module holds a loop and a
  divide or remainder with a constant operand.
- The run first removes the unroll-disable mark from loop metadata
  of functions containing such a divide, then unrolls at a raised
  threshold and cleans up (`sccp`/`adce` collapse the constant
  chain the unroll leaves).
- The folded text ships only when its instruction count is
  strictly below the base text's. A runtime loop that unrolls
  into more code, or a failed `opt` run, keeps the base output.

## Rationale

The guard makes the stage self-limiting: it can only replace a
loop with provably less IR, so no per-program tuning or baseline
exception is needed. Scoping the metadata edit to constant-divide
functions keeps unrelated loops from unrolling at all. `-O0`
skips the stage because its contract leaves control flow
untouched for stepping and bisection.

## Rejected alternatives

- Adding `loop-unroll` to the base list: the disable marks make
  it a no-op on `-O1` IR, and without the guard a raised
  threshold unrolls runtime loops into flash regressions.
- Replacing the marks with `llvm.loop.unroll.full`: forces every
  loop in the function to expand fully, whatever its trip count,
  where the threshold plus guard already fold the constant case.
