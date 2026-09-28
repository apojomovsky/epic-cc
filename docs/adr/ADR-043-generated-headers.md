# ADR-043 -- Generated device headers and a real xc.h

**Status:** Accepted 2026-09-26 (user-approved design in epic-cc#688;
bare-bit deferral user-approved 2026-09-26)<br>
**Decides:** `epic-cc#688`<br>
**Evidence:** docs/46 D-3; `crates/driver/src/headers.rs`;
`crates/driver/tests/xc_headers_e2e.rs` (blink per beta core in the sim)

## Context

Tutorial code names registers and bits the XC8 way (`PORTB`,
`PORTBbits.RB0`, `_XTAL_FREQ`, `__delay_ms`, `__interrupt()`), but
epic-cc shipped an empty `xc.h` stub and no per-device headers. The
SFR/bitfield tables already exist in the device registry (epic-cc#687),
so the headers generate from data, not from hand transcription.

## Decision

- **The driver generates `pic<part>.h` from the registry's `sfrs`** for
  the part under compilation, and ships a real `<xc.h>` that includes it
  by part macro (`_16F877A`, the macro the driver predefines). Parts
  without a table get a per-part `#error` naming them; an unrecognized
  macro falls through to a generic one.
- **Registers are absolute-address dereferences**
  (`*(volatile unsigned char *)`), 16-bit joined registers
  `uint16_t`, 3-byte PIC18 joined registers skipped with their bytes
  kept. Bits are `<REG>bits` unions of `unsigned char` bitfields, one
  struct per pack mode, gaps padded; a member colliding with a header
  macro (the pack names a field `PCLATH` inside register `PCLATH`) is
  skipped with the cursor still advanced.
- **Legacy aliases** (`DDRA` for `TRISA`) cover the register and its
  bits variable. Headers carry the pack attribution (DFP data,
  Apache-2.0).
- **`__interrupt(...)` is one variadic predefine.** Empty means 0 via
  `__VA_OPT__` (`interrupt(0)` when no spelling is present, `+ 0`
  otherwise, which clang folds before the attribute is read);
  `high_priority`/`low_priority` ride as PIC18-only defines expanding
  to 1/2, so the numeric, empty and priority spellings all reach
  `__attribute__((interrupt(...)))` through the same macro.
- **`__delay_ms`/`__delay_us` expand to `_delay(cycles)`** with the
  Fosc/4000 (resp. Fosc/4000000) instruction-cycle arithmetic over the
  user's `_XTAL_FREQ`.
- **The XC8 predefine set stays** (epic-hal relies on it) with
  `__EPIC_CC__` added, and `--dump-include-dir` writes every device
  header, so the on-disk bundle of epic-cc#690 carries the generated
  set.

## Rejected alternatives

- **Bare bit macros (`#define RB0 PORTBbits.RB0`).** Any object-like
  macro named `RB0` also expands after `.`, so qualified access becomes
  `PORTBbits.PORTBbits.RB0` and clang rejects it (reproduced
  in-container). Renaming cannot help; the collision is inherent to
  textual macros. Bare names wait for a real bit-addressable entity
  (epic-cc#707). The headers keep the `#ifndef NO_BIT_DEFINES` guard so
  the opt-out spelling stays reserved.
- **Hand-written per-device headers.** Six beta parts today, more later;
  transcription drifts from the registry, generation cannot.

## Revisit if

A bit entity lands (#707 restores bare names behind the guard), or a
new core needs a header shape this one cannot express (wider joined
registers, a second bits-naming scheme).
