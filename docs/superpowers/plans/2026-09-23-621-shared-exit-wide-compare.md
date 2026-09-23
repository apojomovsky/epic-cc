# Plan: epic-cc#621 shared-exit wide compare

Status: design, waiting on approval. Branch `perf/621-wide-compare`, base
`origin/master` (885fe58).

## Goal

`bench-u32-loop` is 105 flash words against XC8's 49, and the
`wide-compare-branch` cluster is 150 words on the menu-demo ladder entry.
Lower multi-byte variable compares to one borrow chain with a single
taken/not-taken exit, and route that exit straight to the consuming
branch when the compare result has no other use.

## Evidence

Measured on `origin/master` by emitting the listing and reading it
(`scripts/density-profile.py` plus a hand count, both reproduced for this
design):

- `bench-u32-loop`: 105 words = `main` 94 + `__start` 9 + prologue 2.
  Inside `main`, the compare shapes are 36 of the 94 words:
  the 4-lane bound check (`MOVF`/`SUBWF` plus `BNC`/`BZ`/`BRA` per lane,
  20 words) with its preclear, materialize and reload (`CLRF`, `INCF`,
  `MOVF`, `BZ`, 4 words), 24 in total, and the 4-lane eq-zero guard
  (8 words) with the same 4-word tail, 13 in total.
- menu-demo: 11 multi-lane compares, 5 of 2 lanes and 6 of 4 lanes, 150
  words of lanes and exits, 27 more words of materialize and reload.
- XC8 row reproduced for this design: `xc8-cc -mcpu=18f4550 -O2` on the
  same source gives 98 program-space bytes = 49 words, of which 45 are
  code at `0x800` and 4 are the reset vector and config word. The row in
  `fixtures/xc8_reference.md` stands.

The chain replaces each lane's three-instruction exit (`BNC`, `BZ`,
`BRA`) with nothing: the borrow propagates through `SUBWFB` and only the
last lane branches.

## Construction

### Borrow chain, unsigned predicates

`a < b` for `n` lanes becomes a low-to-high borrow chain whose final
carry is the answer:

```
    MOVF b{0},W,A        ; W = b low lane
    SUBWF a{0},W,A       ; C = (a0 >= b0)
    MOVF b{1},W,A
    SUBWFB a{1},W,A      ; W = a1 - b1 - !C
    MOVF b{2},W,A
    SUBWFB a{2},W,A
    MOVF b{3},W,A
    SUBWFB a{3},W,A      ; C = (a >= b)
```

The direction is load-bearing: a borrow propagates upward, so the chain
must start at lane 0. High-to-low compares each lane against a borrow
that has not happened yet and is wrong on 0x0100 against 0x00FF.

`ult`/`uge` read that carry directly. `ugt`/`ule` are the same chain with
the operands swapped (compare `b < a`) and the branch polarity flipped,
which keeps the shared exit for the non-strict predicates too. Cost is
`2n` words plus 2 for the exit, against today's `2n` plus `3n`: 10 against
20 at four lanes, 6 against 10 at two.

Signed ordering predicates stay on today's high-to-low cascade. The
signed answer needs the sign-equality of the top lane, which the borrow
chain's single final flag does not carry; fusing them to the branch arms
still drops the materialize and reload below.

### Fusion into the consuming branch

`emit_icmp_i32`/`emit_icmp_i16` currently materialize a 0/1 byte into the
compare's destination slot, and the `BrCond` that consumes it reloads
that byte and tests it (`MOVF` then `BZ`). When the destination is read
by exactly one instruction and that instruction is a `BrCond` in the same
function, emit the compare's exits as the branch's own targets instead.
The phi copies the `BrCond` arm already emits per edge move to the two
exit labels, so the `(Some(ct), Some(cf))` and single-edge cases keep
working.

This removes the preclear `CLRF`, the `INCF`, and the reload pair from
each site: 4 words on the bench's bound check and eq-zero guard, 27 words
over the menu-demo sites.

A local use scan over the function's instruction list is enough for the
single-use test; `alloc`'s liveness data is not reachable from
`isel-pic18` and does not need to be.

## Expected result

| shape | today | after | delta |
|---|---|---|---|
| 4-lane `ult` bound check, bench-u32-loop | 24 | 10 | -14 |
| 4-lane `eq` zero guard, bench-u32-loop | 13 | 9 | -4 |
| bench-u32-loop flash | 105 | 87 | -18 |
| menu-demo `wide-compare-branch` | 150 + 27 | about 90 | about -87 |

The menu-demo figure is the fusion applied to every site; sites whose
compare leaves the function or feeds a non-branch consumer keep today's
lowering and are counted as unchanged.

## What this does not reach

`bench-u32-loop` does not land at XC8's 49 words on this change. The
remaining distance is not compare shape:

- 24 words are `MOVFF` staging between globals and local slots (the
  `limit` load, the loop counter write-back, and the two `tick`
  read-modify-writes). W-tracking across a store and reload is
  epic-cc#502's scope.
- 12 words are the 32-bit increment lowered as four `MOVLW`/`ADDWF`/
  `MOVWF` groups (`wide-literal-arith`).
- 12 words are the `__start` zero-fill loop and prologue, which XC8 does
  not carry at this size.

Compare shape is 24 of the 105 words. Reaching 49 needs the staged copies
and the increment width as well, which are separate tickets with their
own areas.

## Verification

1. `crates/superopt/tests/`: the flag contract of the chain, verified
   against `sim::Pic18`. The candidate alphabet's straight-line subset
   cannot express a labelled branch, so the verifiable contract is the
   chain prefix: after the last `SUBWFB`, `C` is exactly `(a >= b)`.
   Case set: every byte lane swept through 0..255 for i16 (65536 pairs),
   and the derived-pattern sweep plus per-lane sweep used for the 32-bit
   forms (`crates/superopt/tests/shift_32bit.rs` set the precedent for a
   curated 32-bit sample rather than the full 2^64 pair domain).
2. `crates/isel-pic18/tests/isel_pic18.rs`: simulate the selector's own
   emitted asm for each fused predicate at i16 and i32, the way the
   landed shift forms are re-checked, so the branches are covered and not
   just the prefix.
3. Existing shape tests: the four pins listed in the scout map
   (`i32_icmp_eq_ne_compare_all_four_bytes` 143-147,
   `i32_icmp_ugt_compares_high_byte_first` 167,
   `icmp_i32_eq_zero_converts_every_lane` 5482-5483,
   `icmp_i16_eq_zero_compares_the_high_byte_with_movf_but_keeps_its_subtract`
   5560-5570) move to the new shape. The materialization tests
   (`icmp_result_needs_no_literal_diamond` and the preclear and aliasing
   tests) keep their subject: the non-fused path is unchanged.
4. Ladder: `bench-u32-loop` baseline drops with
   `UPDATE_SIZE_BASELINE=1`, `hal-pic18-menu-demo-18f4550` too, and the
   diff is read to confirm no other entry moved.
5. Full suite in the dev image, sim gates included; `scripts/lint-python.sh`
   clean; `make pre-pr-check` clean.

## Risks

- The fusion changes which label a `BrCond` branch lands on, so a phi
  copy emitted on the wrong edge silently corrupts a loop header. The
  fused tests simulate the emitted sequences per predicate rather than
  asserting text, which is what catches that class.
- The `BrCond` arm asserts on a constant condition today; the fused path
  inherits the assert and never sees a constant (`icmp` operands are not
  constants after `legalize`'s folding).
- Signed predicates keep two exits in places where the chain would need
  one; the menu-demo saving is therefore a range, and the measured number
  goes in the pull request body rather than the estimate here.

## Rejected alternatives

- Materialize into the destination and let a later pass remove it. No
  PIC18 peephole pass exists (`main.rs` runs isel straight to asm for
  this core), and adding one for a single shape is a larger surface than
  doing it at the compare.
- Equality accumulation for the non-strict predicates (`CLRF` plus
  `IORWF` per lane). Swapping the operands and flipping the polarity is
  the same answer in fewer words, because `a <= b` is `!(b < a)`.
- Keep the materialization and only shorten the lane exits. That is the
  half of the win that the reload pair and the preclear hold.

## Decision needed

`bench-u32-loop` lands at about 87 words, not at the XC8 row of 49. The
ladder acceptance line says at or below 49. Options:

1. Land the compare change, record about 87, and file the staging and
   increment-width follow-ups (one of which is epic-cc#502, already on
   the board). The menu-demo cluster line is met as written.
2. Widen this ticket to include the staging elimination. That is
   epic-cc#502's scope and its `area:codegen` label collides with this
   ticket's, so the two cannot be worked at once.
