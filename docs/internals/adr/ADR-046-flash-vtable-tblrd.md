# ADR-046 -- Flash-resident vtables via runtime TBLRD (PIC18)

**Status:** Accepted 2026-10-03<br>
**Decides:** `epic-cc#832`<br>
**Parent:** `docs/internals/design/40-ecpp-subset-design.md` §3 P3, ADR-010, ADR-022

## Decision

* C++ vtables (`_ZTV*`) decode as `is_const` flash tables (ADR-010), not
  RAM copies. The vptr initializer keeps its folded `(vtable, slot
  offset)` ref; `__start` writes it as `LOW(vtable+K)`/`HIGH(vtable+K)`
  link-time literals into the RAM object's vptr field.
* A load through a vptr-derived register lowers on PIC18 to a
  runtime-seeded `TBLRD` sequence: `TBLPTR` loads the reg's two bytes
  (`TBLPTRU` cleared: a vptr value is 16 bits, so a table above 64 KB is
  unrepresentable), each byte reads with `TBLRD*` into `TABLAT` and stages
  to the destination. The outliner may factor the seed; semantics stay
  the seed plus two reads.
* Which registers are vptr-derived is `iselcore::flash_provenance`, shared
  by every backend: a `load ptr` whose source bytes are all covered by
  nonzero-addend refs seeds `flash`; phi/select over all-flash arms stays
  `flash`; a partially flash arm set or a `gep` over a flash reg lands in
  `mixed`. C programs have no nonzero-addend refs, so both sets stay
  empty there and no C lowering changes.
* PIC18 routes `flash` loads to the sequence and panics on `mixed` and on
  stores through either. The PIC14, PIC14E, and baseline backends panic on
  both: C++ virtual dispatch is PIC18-only, and a data-memory read of a
  flash address must never emit silently.
* Amended (epic-cc#1099): the same `mixed` rule covers string literals. A
  phi or select joining a flash literal with any non-walk arm (RAM global,
  null, integer, or pointer param) lands in `mixed`, so a direct load through
  it panics on PIC18 instead of reading INDF0 at a flash address. A
  runtime-indexed load or store through a flash or `mixed` walk phi also
  panics: the check runs on the phi slot, since a dynamic index has no
  provenance key of its own.
* Amended (epic-cc#1100): a constant-length `memcpy` from a `flash` walk reads
  through TBLRD, seeding `TBLPTR` from the walk slot. This covers
  `__builtin_memcpy` and a plain `memcpy` call, which is lowered to the same
  copy because the libc body would read RAM. A `mixed` or runtime-indexed
  source panics. A plain `memcpy` call with a runtime length from a flash walk
  panics. Any memcpy whose destination is a const global, a flash-derived
  pointer, a `mixed` phi, or a runtime-indexed walk panics, whatever its
  source, as the Store arm does. A plain `memcpy` call with such a destination
  or a flash source is lowered to the guarded copy. Open gap: a const-global
  source to a plain call (`memcpy(buf, @table, n)`) still goes to libc. The
  resolver folds a displacement to `u16`, so a walk stepped back (`p[-1]`)
  sign-extends from bit 15; a positive offset of 0x8000 or more reads as
  negative until the resolver carries a signed `i32`.
* The dispatch sequence replays on hardware through the `tblrd-flash-ptr`
  superopt spec (candidate plus `LOW/HIGH` gensym support in the batch
  builder); per-fixture runs stay sim-only per docs/internals/design/40 §5.

## Rationale

The #460 slice proved dispatch with RAM-resident vtables (6 bytes each)
because no lowering existed for a runtime flash dereference: C const
reads always carry a compile-time table base. Seeding `TBLPTR` from the
runtime vptr value closes that gap with one new sequence and one shared
provenance query, reusing the landed `(pos, name, add)` ref channel, the
`LOW(sym+K)` emission, and the ADR-022 compare-and-call chain unchanged.
Panic-on-`mixed` keeps shapes the sequence does not serve (a `gep` over
a vptr, a partially flash select) loud instead of silently wrong.

## Rejected alternatives

* **Keep vtables in RAM** -- correct but permanently spends 6 RAM bytes
  per vtable on a target where RAM is the binding constraint.
* **Per-backend provenance** -- four copies of one rule that must agree
  exactly; a shared query in `iselcore` keeps PIC18's route and the other
  backends' panics consistent by construction.
* **Static-base TBLRD for slot loads** -- the slot address is a runtime
  value (object-dependent); no compile-time base exists.

## Revisit if

A served shape needs `gep`-over-vptr (multi-virtual dispatch adds runtime
slot offsets) or vptr flows through non-global objects (allocas, params);
both currently land in `mixed` or stay unproven and need their own ticket.
Non-global object flows are tracked separately: a vptr value copied
through memory leaves no SSA trail the query can follow.
