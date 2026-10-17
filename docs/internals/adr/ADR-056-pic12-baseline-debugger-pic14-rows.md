# ADR-056 -- PIC12 baseline debugger reuses the PIC14 row walk

**Status:** Accepted 2026-10-17 (coordinator design approval on `epic-cc#1313`)<br>
**Decides:** `epic-cc#1313`<br>
**Evidence:** `crates/driver/tests/debug_session_pic12_e2e.rs`
(`pic12_session_break_print_step` on `p12f509`: break, step, print of
a local and a global, every `--line-table` row agreeing through the
sidecar), `crates/sim/tests/control_baseline.rs` (the `PicBaseline`
control surface), `crates/driver/tests/debug_session_e2e.rs` and
`debug_session_pic18_e2e.rs` (both cores unchanged)

## Context

The PIC14 debugger (doc 34, phases 1-4) walks the raw asm text for line
rows; the PIC18 extension (ADR-055) could not, because its assembler
moves code after isel (far-branch expansion, PCL pads). The baseline
assembler moves nothing: one 12-bit word per text line, a word PC like
PIC14, so the raw-text walk covers it with no new row source. The
driver already threaded `locs` through `isel-pic-baseline` untouched by
later passes (no schedule, banking, or peephole runs on this core), and
the var-table join is core-agnostic.

## Decision

- Baseline rows come from the existing raw-text walk in
  `sidecar::line_rows`; `--line-table` and `.debug_line` share that
  source, so the two artifacts agree by construction, as on PIC14.
- `PicBaseline` grew the phase-3 control surface mirroring `Pic14`
  (`run_until`, `status`/`bank`/`fsr`/`pcl` views, flat physical
  `read_mem`/`write_mem`, `read_prog_word`, `set_w`/`set_pc`/`set_pcl`).
  Memory is flat like PIC18, not bank-resolved like PIC14: DWARF
  addresses name physical GPR storage, so no bank state can skew a span.
- `epic-cc-gdbserver` serves a `PicBaselineTarget` behind `--device`
  with the same i386 session mapping (word PC in `eip`); the one
  baseline-specific slot is `ebx`, which reads PCL and writes it with
  machine semantics (a write pages the PC). The sim takes its file map
  from the named part, defaulting to the P1 target `p12f509`.
- The acceptance fixture keeps its globals volatile: the backend
  forwards a plain global's value past its home store into a temp, so a
  stopped-at line would print the stale home. Volatile pins each store
  home; the same forwarding exists on PIC14 and is not baseline-specific.

## Rejected

- Final-layout rows as on PIC18: nothing moves code after baseline
  isel, so threading loc ids through the assembler buys nothing.
- A bank-resolved `read_mem` as on PIC14: the adapter addresses RAM
  physically, and the baseline bank bit would only add a way to read
  the wrong bank.
