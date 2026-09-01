# Liveness-based intra-frame overlay design

**Status:** Design for `epic-cc#172` (alloc: serial+tick+RB stacks exceed the 368 B GPR space)
**Parent:** `docs/31-ecosystem-integration-design.md` §5 (RAM headroom); `docs/16-integer-spine-m3-plan.md` (deferred liveness reuse)

## Problem

The overlay allocator sizes every function's frame as the **sum of its locals'
byte widths** (M3's "naive per-value demand" baseline). On the 877A's 368 B GPR
space this overflows for the real epic-serial ISR chain and the epic-encoder /
epic-console examples:

| Build | Failure | Globals | main frame today | main frame with liveness |
|---|---|---|---|---|
| combo-tick-serial | `alloc: GPR demand exceeds 0x1EF (0x01f0)` | 194 B | 235 B (130 slots) | 45 B |
| combo-rb-uart | same | 59 B | 298 B depth | ~60 B |
| epic-encoder full example | same | ~50 B | 44 defs | 9 B |
| epic_serial_put_idec | (in the chain) | - | 121 B | 31 B |

The panic fires in the locals phase: `main`'s 130 slots are placed contiguously
from the globals' end, and the walk crosses the last bank. The values are not
simultaneously live: a linear-scan liveness analysis of `main` shows a peak of
~20 slots / 45 bytes. M3 explicitly deferred this: "liveness-based value reuse
within a frame (the naive per-value demand is the conservative baseline)".

## Decision

**Add intra-frame liveness-based slot reuse to `alloc`.** Within each function,
compute every value's live range from the IR, build the interference relation,
and assign slots with a deterministic greedy linear scan: a value reuses the
lowest slot whose previous occupant's range has ended. Frames shrink from
`Σ widths` to the colored peak. The call-graph overlay between functions, the
ISR disjoint region, the runtime-routine single-bank rounding, and the
globals placement are unchanged.

## Design

### D-1: Live ranges from the IR, in `alloc`

`alloc` already reads the whole `Module`; the IR has everything liveness needs:
blocks with instructions, `Phi` incoming edges, `Br`/`BrCond` targets. No new
crate, no new text boundary. The def set is exactly `def_width`'s (the same
values that get map keys today), so the map keys are unchanged; only addresses
move.

Live range model, matching isel's lowering exactly. Each value's live
interval is `[min(def, uses, phi pred ends), max(...)]` in linear block order
(entry first, then label order):

- A value defined in block `B` is live from `B` through the block of its last
  use.
- A **phi destination** is live from the earliest predecessor end (its first
  copy) through its last use: isel eliminates phis by copying each
  predecessor's incoming value into the phi destination at the predecessor's
  end, so the destination must not overlap anything live at any predecessor's
  end. The incoming values are attributed to the predecessor's end too (that
  is where the copy reads them), not to the merge block.
- **Params** are live from function entry to their last use.
- A **loop-carried value** (use before def in linear order) gets an interval
  spanning the loop, so it can never alias a value it is co-live with. This
  is what makes the model sound on back edges: a naive block-index range
  (`def..last use`) inverts for loop-carried values and lets phi-dst copies
  swap slots (the `epic_tick_init` cyclic-phi miscompile the interval model
  prevents).
- Values with no use (dead defs) have a point interval at their def: they
  still get a slot (isel reads every def's address from the map), but the
  slot is immediately reusable.

### D-2: Greedy linear-scan coloring, deterministic

Sort values by (range start, placement order) for determinism: params first,
then defined values in instruction order, so same-start values keep the
documented layout order. Maintain the placed slots as `(interval, width)`
with the interval merged across occupants. For each value, reuse the lowest
slot whose merged interval is disjoint from the new value's; otherwise
allocate a new slot. A reused slot keeps its address and its width grows to
the widest occupant; the frame's physical end is the walk of the distinct
slot widths through `place_contiguous` (bank stepping and region-tail holes
preserved).

No alignment constraint beyond what exists today: `place_contiguous` does not
even-align locals (it relies on region starts being even), and isel is
byte-addressed, so an odd i16 is already legal. The liveness version is no
stricter.

### D-3: Frame size and `frame_end` stay base-independent

The coloring depends only on the function's IR, not on its base, so it is
computed once per function before the base-assignment loop:

- `frame_size(f)` = the colored peak in bytes (replaces `locals_size(f)` in
  `depth_end`).
- `frame_end(f)` walks the **distinct slot widths in allocation order** through
  `place_contiguous`, exactly as it walks `locals_widths` today. Shared slots
  are walked once. This keeps the base computation (step 6) and the ISR region
  (step 6b) exact: a callee's base derives from the caller's true physical end,
  region-tail holes included.
- The locals placement (step 7) places each value at its slot's address; the
  walk and the placement agree by construction.

### D-4: What does not change

- Globals: sequential + bin-pack fallback, untouched.
- Call-graph overlay: `base(f) = max over callers of frame_end(caller)`,
  roots at `bank0_start`, unchanged.
- ISR disjoint region: computed from the non-ISR roots' physical ends,
  unchanged.
- Runtime routines: `round_if_routine` single-bank rounding, unchanged.
- `AllocLayout`, `map_text`, the driver's map/size report: unchanged (the
  report's "overlay" definition already covers a byte live in several frames).
- isel: reads every address from the map; no isel change.

## Correctness argument

- **No overlap of live values**: the coloring only reuses a slot whose
  occupant's range ended before the new value's range starts; interference is
  exact by construction.
- **Phi semantics match isel**: phi destinations are live at every predecessor
  end, so a reused slot never holds a phi destination and a predecessor-live
  value simultaneously.
- **Physical placement matches the walk**: both use `place_contiguous` over
  the same slot widths in the same order, so `frame_end` (used for callee
  bases and the ISR base) equals the true placed end.
- **Determinism**: sorted iteration everywhere; the same module always colors
  identically.

## Verification

- **Alloc unit tests**: two non-interfering values share a slot; interfering
  values do not; a phi destination is live at predecessor ends; a dead def's
  slot is immediately reusable; frame size equals the colored peak, not the
  width sum; determinism (same input, same map).
- **Existing alloc tests**: address-asserting tests for non-reusing modules
  keep their addresses (no interference, no reuse, placement order unchanged
  for the first slot); tests whose modules have reuse get updated addresses
  with the new expected values.
- **Acceptance builds** (epic-hal, via `make epiccc-build`): the epic-encoder
  sizecheck variant links on 16F877A and 16F887 with headroom recorded
  (RAM 69/368 vs 150/368 on master, flash 1649/8192 vs 1785/8192). The full
  examples (epic-encoder, epic-serial, the combos with the target harness)
  now pass the allocator but hit a pre-existing flash-density wall (10.7K
  words vs the 877A's 8K flash) — a codegen issue separate from this ticket,
  newly exposed. The epic-console example additionally needs its epic-hal
  put_str literal fix (see below).
- **Full suite**: `cargo test --workspace` (the CI gate), plus the epic-hal
  epiccc-gate path where it applies.
- **Doc 31 §5**: the RAM-headroom item updated with the outcome (ticket
  acceptance item 3).

## Out of scope

- **epic-encoder-tick combo's global bin-pack failure** (377 B of globals,
  five `EPIC_HARNESS_LOG_STATIC` staging buffers owning 291 B): a globals
  problem, not a locals problem; not in this ticket's acceptance. Recorded in
  doc 31 as a separate epic-hal staging-buffer reduction.
- **epic-console's epic-cc compile break**: `epic_serial_put_str(s_txt_led_on)`
  with a variable is invalid under the #117 literal-only macro. A separate
  epic-hal change (literal call sites) restores the ticket-time baseline; the
  allocator fix then lets it link.
- **PIC18 / i64 gaps** (the 18F4550 encoder path): explicitly out of scope in
  the ticket.

## Risks

- **Liveness must match isel's phi-copy semantics exactly.** The model above
  accounts for it; the unit tests pin it.
- **Address churn for existing programs.** Expected: M3 deferred this, and the
  ticket is that deferral. Tests that assert exact addresses for reusing
  modules are updated; the map/size report contract is unchanged.
- **Frame-end walk vs placement drift.** Both use the same slot widths in the
  same order; a unit test asserts `frame_end` equals the placed end for a
  bank-crossing frame.
