# ADR-055 -- PIC18 debugger rows come from the assembler's final layout

**Status:** Accepted 2026-10-17 (coordinator design approval on `epic-cc#1312`)<br>
**Decides:** `epic-cc#1312`<br>
**Evidence:** `crates/driver/tests/debug_session_pic18_e2e.rs`
(`pic18_session_break_print_step` on `p18f4550`: break, step, print of
a local, a global, and a struct member; every `--line-table` row agrees
through the sidecar), `crates/driver/tests/debug_session_e2e.rs`
(PIC14 sessions unchanged)

## Context

The PIC14 debugger (doc 34, phases 1-4) walks the raw asm text for line
rows: one word per line, word addresses matching the PIC14 PC. PIC18
breaks both assumptions. Its PC counts bytes, `GOTO`/`CALL`/`LFSR`/`MOVFF`
take two words, and the assembler itself moves code after isel: far
conditional branches expand in place and PCL computed-jump tables pad
NOPs at their anchor. A raw-text walk would misreport every address past
the first moved line, and breakpoints would land on neighboring code.

## Decision

- `sidecar::line_rows` takes the core. PIC14-family rows walk the raw
  text as before; PIC18 rows come from `asm::assemble_pic18_with_locs`,
  which threads opaque caller loc ids through the stream build,
  far-branch splices (inheriting the replaced line's id), and PCL pads
  (no id), emitting `(id, byte address)` from the final layout.
- `--line-table` and `.debug_line` still share one row source, so the
  two artifacts agree by construction on both cores.
- The var-table join is reused unchanged.
- The simulator and adapter mirror PIC14 per core semantics (see doc
  34 §5).

## Scope note

Rows stay `u16`, so parts past 32K words of flash (catalog giants
such as `p18f6720`) sit outside the debuggable scope until the row
type grows; every part up to 32K words, including `p18f4550`, fits.

## Rejected

- Byte addresses by `words * 2` over the raw text: exact until the
  first far-branch expansion or PCL pad, silently wrong after.
- A custom gdb target description: stock gdb rejects it (doc 34 §3
  spike); the i386 mapping stays, with its known step-over-call
  limit, now filed as a follow-up.
