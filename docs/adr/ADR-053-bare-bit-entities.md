# ADR-053 -- Bare bit names as section-placed `_Bool` entities

**Status:** Accepted 2026-10-14 (oracle decision on epic-cc#707)<br>
**Decides:** `epic-cc#707`<br>
**Evidence:** `crates/driver/tests/bare_bits_e2e.rs` (sim SFR state plus
`BSF`/`BCF`/`BTFSC` asm pins, bank-1 select, `&GIE` refusal),
`crates/irparse/tests/placement.rs` (section retention),
in-container clang probes showing `__bit` unknown, `_BitInt` rejected on
msp430, and `address()` ignored

## Context

Tutorial code writes `GIE = 1`, but any object-like macro named `RB0`
also expands after `.`, so `PORTBbits.RB0` breaks (ADR-043, #688). The
pinned clang 20.1.8 offers no bit type and no absolute placement, and
forking it is out of scope. The compiler must therefore bind each bare
name to its (register, bit) itself.

## Decision

- Headers declare one entity per single-bit field on PIC14:
  `extern volatile __epic_bit_t NAME` (`_Bool` in C, `bool` in C++)
  with `section(".epic.bit.<addr>.<bit>")`, inside the existing
  `#ifndef NO_BIT_DEFINES` guard. Multi-bit fields and taken names get
  no declaration; other cores keep the empty guard.
- irparse retains a section-carrying `external` as a `Global` whose
  `addr` is `Placed { byte, bit: Some }`. The section is the same
  carrier `EPIC_AT`/`.epicat.` already uses, so the `.ll` stays the
  only artifact between stages.
- `alloc` places no RAM for bit entities and ignores their SFR
  addresses in layout fallback; folds never touch them (not RAM, no
  forwarding into a bit store).
- isel-PIC14 lowers constant stores to `BCF`/`BSF`, loads to a
  skip-safe `BTFSC` materializing a 0/1 byte, and computed stores
  (`^= 1` included) to a value test plus set/clear. Any address
  escape panics naming the entity.

## Rationale

The shape matches every public prior art without copying any: SDCC
pairs `__bit` with `__at` plus the `extern` split declaration, XC8
documents bit types with the `address` attribute, CCS binds with
`#bit`. On the beta PIC14 parts every single-bit name is unique with
no macro or keyword collision, so PIC14 needs no binding rule.

## Rejected alternatives

- **Object-like or function-like macros:** proven broken (#688), and
  the call form misses the tutorial spelling.
- **Driver sidecar map:** a second artifact beside the `.ll` breaks
  the diffable-text-boundary rule.
- **Clang fork for `__bit`:** the pin is a migration, never
  housekeeping.
- **Struct-path RMW fusion instead:** leaves the ticket's spelling
  unmet; tracked as #1281.

## Revisit if

PIC18 takes bare names (its 42 shared names, `RB0` on PORTB and
TRISB, need a binding rule first), or a part names two sites alike
(the generator panics on conflicting bindings today).
