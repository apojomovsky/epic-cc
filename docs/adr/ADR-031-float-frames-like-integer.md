# ADR-031 -- Float frames place like integer ones

**Status:** Accepted 2026-09-10<br>
**Decides:** `epic-cc#357` (cross-context float window sharing)<br>
**Parent:** `docs/adr/ADR-015-pic18-softfloat.md` (the access-bank frame rule this supersedes)

## Decision

* Float runtime routines lose their special placement: `alloc` treats them
  exactly like integer routines (`routine_base`'s single-GPR-bank rounding,
  bases relative to their own context's region). The access-bank window
  reservation (`access_window`/`float_routine_base`) is deleted; globals
  and frames pack from the device's GPR start again.
* The float recipes address memory through a new `emit_banked` helper
  (`operand()`'s MOVLB discipline, per-`Gen` BSR tracking) instead of
  hardcoded `,A`. Integer routines already did this.
* Because routine copies are per context (ADR-030's regions), each ISR
  priority's float copy now owns a disjoint frame: an interrupt preempts
  main mid-float-op without touching main's operands or scratch.
* The ISR epilogue restore order is fixed in passing (found by this
  work's e2e): with the old aliased fixed-block layout the retval backup
  clobbered the STATUS/BSR/FSR0L snapshots before they were read; the
  snapshots now live in non-aliased slots (TBLPTR 0x005-0x007, FSR0H
  0x004, W 0x008, STATUS/BSR/FSR0L 0x009-0x00B, retval backup 0x00C-
  0x00F) and the epilogue restores SFRs before the retval backup.

## Rationale

ADR-015's real invariant was narrower than its rule: what the recipes
need is "no MOVLB between a skip test and its target". Auditing every
skip in the float bodies shows each targets the very next instruction or
an explicit GOTO, so a MOVLB can never land inside a skip window and
banked frames are sound. Pinning every float frame (base plus ISR copies)
to one window start made all contexts share scratch, so any interrupt
preempting main mid-float-op silently corrupted main's computation.

## Rejected alternatives

* Partitioning the access-bank window by context: fits three contexts
  today, caps at four, and couples `alloc`'s region scheme to a 96-byte
  window. Rejected in favor of the general mechanism.
* Keep `,A` and only move frames: does not fix the hazard; the recipes
  would still be unable to reach frames outside the window.

## Consequences

* Float programs no longer shift their globals above the access-bank
  window (smaller footprint; addresses change).
* `emit_f32_*` helpers emit `,A` or `,B` per `operand()`; compat ISR
  emission changes bytes (shared with the #363 W-slot fix).
* Compat-mode save layout moves (SFR snapshots to 0x009-0x00B): recorded
  SDCC-parity baselines see different ISR prologue/epilogue bytes.
