# ADR-032 -- Baseline const tables live in page low halves, readers cost a stack level

**Status:** Accepted 2026-09-10<br>
**Decides:** `epic-cc#327` (pic-baseline P4: const in flash via RETLW)<br>
**Parent:** `docs/37-pic-baseline-port-design.md` D-5 (the 256-word ceiling this implements)

## Decision

* The const section packs the page-0 low half first and spills to the
  page-1 low half behind an `org`. Tables sort by page before printing,
  so one page-1 `org` serves every spill. A table fitting neither half
  (over 252 bytes with its 4-word reader) is rejected loudly, never
  chunked: no 256-byte window on this core can hold it.
* Reader entries are 4 words (`MOVWF scratch / MOVLW base / ADDWF
  scratch,W / MOVWF PCL`, D-6 needs no `ADDLW`) with numeric bases;
  call sites set PA0 to the table page, `CALL`, and restore PA0 to the
  caller page. `verify_page_fit` audits placement against the text,
  including the reader immediates.
* A const read reserves one stack level: with a read present, deepest
  frame plus `__start -> main` plus the reader must fit the silicon
  stack, enforced in the driver and mirrored in tests. Depth-2 programs
  that read consts do not compile on the 2-level 509.
* The `asm` encoder accepts any page low half as a `CALL` target (the
  literal is page-independent); PA0/target agreement stays purely with
  the page-fit audit, which alone sees the PA0 dataflow.

## Rejected alternatives

* Chunked multi-window tables (classic `isel`'s `>= 256` shape): every
  chunk needs its own low half, and a 509 program with room for chunks
  has room to keep tables whole. Revisit if a table over 252 bytes must
  ever compile for this core.
* Encoder-side PA0 tracking: the encoder sees one line at a time, so it
  cannot know the selecting PA0. Kept encodability (low half) in the
  encoder, correctness (right page) in the audit.
