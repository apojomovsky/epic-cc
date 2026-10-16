# ADR-051 -- Short const-table readers share their base's page

**Status:** Accepted 2026-10-12<br>
**Decides:** `epic-cc#1080`<br>
**Follow-up:** `epic-cc#1188` (port to `isel-pic14e`)<br>
**Evidence:** `crates/isel/tests/isel.rs`
(`same_page_const_read_skips_restore`,
`small_const_table_in_nonzero_window_reads_correctly`,
`large_const_table_emits_two_entry_reader_and_16bit_caller`)

## Context

Every `const` table read CALLs a `__read_<name>` entry whose 6-word
body parked the index in scratch across its own `MOVLW HIGH(base)` /
`MOVWF PCLATH` set. The set exists because `MOVLW` clobbers W, and the
park because the index arrives in W. Twelve such parks remain on the
16F877A encoder listing (#1027 survey, rank 5).

The caller's pre-CALL set already loads PCLATH, but only `PAGE(entry)`:
bits 4:3 for the CALL. The low page bits stay stale, so the entry could
not rely on them and rewrote the whole register.

## Decision

- An entry whose base lands in its own 2K page goes short: `ADDLW
  LOW(base); MOVWF PCL`, two words, no park, no PCLATH write.
- Its callers set the base's full `HIGH(base)` ahead of the CALL. The
  CALL reads bits 4:3 (the entry's page, equal by placement) and the
  entry's jump inherits the rest.
- Placement carries the short lengths, so modeled and emitted addresses
  agree. Pass-B callers read a short-entry map built from the same
  placement. A measured start that would decide any entry differently
  re-anchors the section to the modeled start, like a page drift.
- Entries whose base sits in another page keep the 6-word body and the
  `PAGE(entry)` caller set.

## Rationale

The full `HIGH(base)` set costs the caller nothing (one word either
way); the saving is the entry's four words per same-page table. Twelve
sites on the surveyed listing, all same-page. The map, not a new
calling convention, keeps the contract local: an entry absent from the
map behaves exactly as before. The driver re-checks every modeled base
page after its own passes (cc#1195), so a repair pad that drifted a
base across a page fails loudly instead of silently breaking a pair.

## Rejected alternatives

- Same-window shortness without a caller change: the caller set leaves
  `PCLATH<2:0>` zeroed, so only a window-aligned base could skip the
  set. Setting other low bits with `BSF`/`BCF` costs more than the park.
- Deciding shortness in the window-repair pass: the callers are
  out of its reach, and their sets must change together with the entry.
