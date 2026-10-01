# ADR-045 -- Optimization profiles `-O0`/`-O1`/`-O2`/`-Os`

**Status:** Accepted 2026-09-30<br>
**Decides:** `epic-cc#839` (track F of `epic-cc#835`)<br>
**Evidence:** scratch measurements in the PR (menu demo, encoder, struct-scan
benches on both cores); the per-profile size baselines in
`crates/driver/tests/fixtures/size_baseline.toml`

## Context

`epic-cc` had one fixed tradeoff: size first. PIC18 code factoring (ADR-042,
`--no-outline` to disable) and the always-inline policy in `wholeprog_opt`
are tuned for flash. Speed work needs choices that cost flash, and CI's
`STRICT_SIZE_BASELINE` would reject every one of them. A user-selectable
profile is both the product feature (pick size or speed) and the
precondition for the speed track. The flag spellings match what PIC users
already type.

## Decision

- **Four profiles on the driver, `-Os` the default.** `-Os` runs today's
  pipeline exactly and produces byte-identical HEX on every ladder row
  (proven by the unchanged `-Os` baselines). Profiles select our passes
  only: clang stays pinned at `-O1` under every profile, since its version
  is part of our input format.
- **`-O0` folds whole-program constants and restructures nothing**
  (`internalize`, `ipsccp`, local cleanup; no `loop-reduce`, no folding,
  no factoring). Skipping `opt` entirely was measured and rejected: the
  menu demo then reaches 18227 words and no longer fits the 18F4550's
  16384 words of flash, so a `-O0` that builds nothing real is no use to
  the debugger track. With constants folded it builds everything
  (menu `-O0` is 11165 words) while the IR keeps the source's control
  flow for stepping and bisection.
- **`-O1` runs the base pass list with no cross-function folding,**
  factoring still on. It bisects exactly the always-inline step of `-Os`
  (menu: 9672 vs 9006 words).
- **`-O2` is speed: no code factoring, and the aggressive inline tier.**
  Single-call-site callees fold even into `main`/ISR roots, capped at
  100 IR lines per callee so a large single-use driver never pins its
  whole frame permanently in a root that never returns. Measured, not
  assumed: menu `-O2` is 10291 flash (+1285, nearly all the un-factored
  1289) and 785 RAM (+30); encoder `-O2` is 5346 flash (-102, PIC14 call
  overhead outruns the fold) and 339 RAM (+13). Later speed levers land
  behind `-O2`.
- **Fine-grained switches stay as overrides on top of a profile.**
  `--no-outline` forces factoring off under any profile (a no-op under
  `-O0`/`-O2`, where it is already off). `--const-pool` is orthogonal
  and unchanged.
- **The size ladder builds every row under `-O2` and gates `-O2` numbers
  on a subset.** `-O2` must build the whole ladder (a profile that
  breaks a program is a bug), but gating `-O2` numbers on every row
  would double the strict number surface each speed lever must
  re-baseline. The gated subset is the smallest and largest row on each
  core plus the loop-shape bench that pins LSR under `-O2`
  (`add-16f877a`, `add-18f4550`, both `bench-struct-scan` rows, the
  encoder and menu rows). Baseline entries carry `profile`; rows without
  one are `-Os`, so the existing rows are untouched. `STRICT` applies to
  each gated profile independently. `make size-report` still reads the
  `-Os` numbers only.
- **Correctness per profile.** A sim matrix test (`opt_profile_e2e`)
  builds one call- and loop-heavy fixture under all four profiles on
  both cores and pins one checksum. The differential fuzz gate runs the
  fast integer seeds under `-O2` on PIC14 and PIC18; float, signed and
  IR corpora under `-O2`, and the full corpora, ride in a follow-up.
- **The `--report` JSON gains `opt_level`.** ADR-025 still defines every
  other key; this ADR adds the one key that names the profile that
  built the report, so builders (epic-hal, PlatformIO) can record it.

## Consequences

- `-Os` output is identical to pre-change on all rows: the ladder passes
  with no `-Os` baseline touched, and a baseline-vs-branch driver diff
  (383bae0 vs this change, default flags) is byte-identical HEX on the
  menu and encoder demos plus five small rows.
- `-O2` costs flash on factoring-friendly PIC18 programs and can cost
  RAM anywhere the aggressive tier fires; both are baselined, not
  regretted.
- The cycle ladder (#840, built in parallel) makes the `-Os`/`-O2`
  cycle baselines profile-aware, whichever lands second.

## Rejected alternatives

- **`-O0`/`-O1` as documented aliases of `-Os`.** Real reduced pipelines
  are more useful: each profile bisects exactly one shaping step, which
  is what the debugger track and miscompile bisection need.
- **`-O0` skipping `opt` entirely.** Rejected by measurement (menu
  18227 words, does not fit). The fold-only pipeline keeps every row
  building.
- **Gating `-O2` numbers on all rows.** Same compiles, but every speed
  lever would re-baseline the whole ladder. The subset keeps the strict
  surface where the signal is.
