# ADR-009: PIC18 pointer model (shared GEP fold, single FSR0, no PLUSWn)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p3-pointers)

## Decision

PIC18 pointer/array/struct support (port P3) uses:

1. **A shared GEP-fold resolver in `iselcore`.** `resolve_pointers(m)`
   folds every `gep` chain (constant offsets summed, dynamic terms
   collected, bases resolved through chains) into a
   `HashMap<{func}::{reg}, (Base, k, terms)>` consumed by both backends.
   PIC14's `isel` and PIC18's `isel-pic18` call the same function; the
   fold algorithm moved verbatim, only its crate changed.
2. **A two-case pointer model.** A resolved pointer is either `Direct`
   (statically known address: plain `MOVFF`/`MOVF`/`MOVWF`) or
   `Indirect` (FSR0 set up, access through `INDF0`). An sret param's
   slot holds a 2-byte target ADDRESS, not the object itself
   (`Base::Slot(name, true)`), and always goes indirect.
3. **Exactly one indirection register, FSR0, with per-byte re-setup.**
   Every dynamic access recomputes `FSR0 = base + k + Σ scale×%reg +
   byte_off` from scratch per byte (`LFSR` for the static part, unrolled
   `ADDWF`/`ADDWFC` for the dynamic term). No FSR auto-increment, no
   second FSR.
4. **No `PLUSWn` for dynamic-offset writes.** `PLUSWn` computes its
   effective address from `FSRn + W` at execution time; a write needs `W`
   to hold the byte being stored, colliding with using `W` as the offset.
   All dynamic accesses physically advance `FSRnL`/`FSRnH` and go through
   plain `INDFn`, for reads and writes alike.
5. **No FSR-window checks.** PIC14's `fsr_window`/`object_span` window
   half exists only because PIC14's four RAM banks are non-contiguous.
   PIC18's `FSRn` is a flat 12-bit register over the whole data space, so
   no object can straddle a boundary that matters. The machinery is not
   ported.
6. **Loud scope boundaries.** One dynamic term per pointer
   (`terms.len() > 1` panics), constant-length `memcpy` only
   (`MemLen::Reg` panics), indirect memcpy source panics, and a plain
   (non-byval, non-sret) pointer parameter dereferenced directly panics
   (`resolve_pointers` only seeds byval/sret params, allocas, and gep
   chains off them; an opaque runtime pointer value handed in by the
   caller has no compile-time base to fold and is not modeled yet).
   Unsupported input aborts with a precise message rather than silently
   miscompiling.
   **Superseded in part by ADR-018:** plain pointer params now resolve,
   so a callee that indexes a caller-supplied pointer compiles on both
   backends. The other boundaries in this item still hold.

## Rationale

- **One fold, two backends.** The GEP fold is subtle (chain folding,
  cyclic-chain detection, byval/sret/alloca seeding); duplicating it in
  `isel-pic18` would have been a second copy of the same bug surface.
  The extraction is behavior-preserving (PIC14 test parity confirmed).
- **Single FSR0 keeps setup a pure function.** Per-byte re-setup costs a
  few extra instructions per multi-byte access but eliminates hidden
  state-ordering dependencies between calls: the same class of implicit
  sequencing assumption the backend already documents against for `BSR`
  tracking. A second FSR would be needed only for two simultaneously
  indirect pointers, which P3's fixtures never require (verified against
  `structs.c`'s compiled IR before implementing).
- **`PLUSWn` is a write hazard, not an optimization.** The plan applies
  the no-`PLUSWn` rule uniformly so a later "optimization" cannot
  reintroduce the write collision by special-casing reads.
- **The IR carries sizes directly.** `object_span` (PIC14's "how big is
  the pointed-to object" query) was planned for P3 but ended up with
  zero production callers: sret copies size by `s.ty.bytes()` and byval
  copies by `arg.byval`. It was deleted rather than shipped dead.

## Rejected alternatives

- **Two FSRs (FSR0 + FSR1) for simultaneous indirect pointers.** More
  registers to track, and no P3 fixture needs it; revisit if a P4+
  program does.
- **FSR auto-increment (`POSTINC0`) for multi-byte accesses.** Implicit
  ordering between setup calls; rejected for the same reason as the
  `BSR`-tracking hazards.
- **Porting PIC14's `fsr_window`/window half of `object_span`.** Dead
  machinery on PIC18's flat address space.

## Revisit if

A P4+ fixture needs two simultaneously indirect pointers (add FSR1), or
the per-byte re-setup shows up in profiling (revisit auto-increment with
an explicit ordering contract).
