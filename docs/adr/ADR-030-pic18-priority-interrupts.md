# ADR-030 -- PIC18 two-vector priority interrupts

**Status:** Accepted 2026-09-09<br>
**Decides:** `epic-cc#346` (PIC18 priority interrupts)<br>
**Parent:** `docs/adr/ADR-013-pic18-interrupts.md` (single-vector compat mode)

## Decision

* Priority partition: `irq_priority == 1` is high, anything else on an
  ISR is low. Priority 0 is the compatibility single-vector marker, never
  a real low-priority handler: a lone handler of any priority keeps the
  ADR-013 wiring (body at vector 0x0008, fixed save block), and only a
  high/low pair switches to priority wiring. This matches IPEN=1 hardware
  routing (priority bit 1 -> high vector, 0 -> low vector).
* Mode validation in `legalize` (fail fast, where priorities are
  understood): at most one handler per explicit priority, at most one
  compat handler, and compat never mixes with explicit priorities. Two
  bodies cannot share one vector entry, so anything else panics instead
  of emitting a silently broken image. `isel-pic18` re-asserts one
  handler per vector plus save-area/pipeline consistency, so a
  hand-built module that skips legalize still fails loudly.
* Duplication is per priority: a helper shared with another live context
  gets that context's own copy (`_isr` for low, `_isr_high` for high),
  with calls, address-taken functions, stored callbacks and runtime
  routines all rewritten per priority. The resolver keeps the epic-cc#137
  visibility rules, partitioned: a site that could run in either ISR
  context with different copies visible panics (no single dispatch serves
  both).
* Three overlay regions in `alloc`: main, low, high. The high region sits
  above everything it can preempt (main and low frames); the low region
  sits above main. Compatibility mode keeps the historical single-region
  layout byte-identical.
* Priority-mode vectors are GOTO stubs (`org 0x0008` -> high body,
  `org 0x0018` -> low body); bodies float after the stubs. Either body
  overflows the 16-byte vector gap (the save prologue alone is 24
  bytes), so body-at-vector cannot work twice. The lone ISR keeps the
  historical body-at-vector layout untouched.
* Save areas: the high ISR reuses the fixed block (same bytes as compat);
  the low ISR gets a 12-byte area at its region's base, below its frames
  (disjoint by construction, carried `alloc` -> `isel-pic18` as
  `isr_low_save`). The new area gives W a dedicated slot; it does not
  copy the fixed block's FSR0H/W slot overlap (epic-cc#356).
* The sim models post-IPEN routing (low vector gated on GIEH+GIEL,
  high entry clears GIEH, low entry clears GIEL only, RETFIE restores the
  returning context's bit). RCON.IPEN itself is firmware-owned (user C
  sets it, as under SDCC/XC8); the compiler never emits it.

## Rationale

Preemption is the whole hazard: the high ISR can interrupt the low one
(and main) mid-call, so any frame or save byte shared across those edges
is a clobber. The three-region + two-save-area layout makes every
preemption edge disjoint by construction, and the e2e
(`priority_irq_e2e`: nested fires with a liveness witness) proves it in
the sim rather than by inspection.

## Rejected alternatives

* Duplicating into the existing single ISR region: the high ISR's frames
  would overlay the low ISR's live frames, the exact clobber this
  feature exists to prevent.
* Fixing the fixed block's FSR0H/W overlap in passing: a 2-line change,
  but it moves compat codegen bytes under a recorded SDCC-parity
  baseline. Filed as epic-cc#356 instead; compat stays byte-identical.
* Per-context float areas: `_isr_high` float copies share the access-bank
  window exactly like `_isr` copies (same pre-existing hazard class, no
  new one). The principled fix covers all three contexts and is filed as
  epic-cc#357.
* Setting RCON.IPEN in `__start`: peripheral configuration is firmware's
  job (SDCC/XC8 behave the same); the compiler emits both vectors and
  the firmware enables priority mode.

## Consequences

* New C surface: `__interrupt(1)` / `__interrupt(2)` (SDCC keyword, rides
  as a `-D` macro over clang's `interrupt(n)` attribute); plain
  `__interrupt` / `interrupt(0)` keeps compat meaning.
* `isel-pic18::select` takes the low save area explicitly; the driver
  forwards `layout.isr_low_save`. A priority pair without one panics.
* PIC14/PIC14E backends are untouched: priority duplication is legalize
  (core-agnostic) but only `isel-pic18` wires the second vector; any
  multi-ISR module for another core fails at that core's existing
  single-vector assertion.
