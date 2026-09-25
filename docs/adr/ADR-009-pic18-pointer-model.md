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
   **Narrowed (epic-cc#473, 2026-09-19):** forwarding a pointer *value*
   as a call argument (a plain-ptr/sret argument whose resolved base is
   an indirect slot at offset 0 with no dynamic terms -- i.e. the
   argument's two bytes are already sitting, fully formed, in a frame
   slot) is neither `Direct` nor a genuine `Indirect` dereference: it is
   a plain 2-byte memory-to-memory copy from that slot into the callee's
   param slot. `emit_call_args` now recognizes this degenerate case
   (`direct_ptr_forward_src`) and copies the two bytes with `MOVFF`
   directly, skipping FSR0 entirely; `emit_ptr_setup`'s FSR0 path is
   reached only when the argument needs a real address computation
   (a GEP or dynamic index).
3. **Exactly one indirection register, FSR0, with per-byte re-setup.**
   Every dynamic access recomputes `FSR0 = base + k + Σ scale×%reg +
   byte_off` from scratch per byte (`LFSR` for the static part, unrolled
   `ADDWF`/`ADDWFC` for the dynamic term). No FSR auto-increment, no
   second FSR.
   **Superseded in part (epic-cc#469, 2026-09-19):** a dynamic term
   whose scale is big enough that a shift-add chain beats its unrolled
   adds is scaled as `scale×idx` from a zero-seeded `LFSR 0, 0x000`
   (doublings plus conditional adds), with the static part re-joining
   as a literal add afterwards. The one-FSR rule still holds.
   **Correction (epic-cc#469 review, 2026-09-19):** the chain's zero
   seed can only hold the running `scale×idx` product, so a `SlotValue`
   origin's own runtime pointer value (unlike `Absolute`'s compile-time
   `base_addr`, which rides the post-chain literal add) cannot ride
   that same add. The first cut of this chain dropped it entirely for
   `emit_fsr0_indirect_slot`/`emit_fsr1_indirect_slot`, landing writes
   near address 0 instead of inside the pointed-to object whenever the
   chain fired through a runtime pointer parameter. Fixed by folding
   the pointer's own two bytes onto the chain-scaled pair with a
   dedicated runtime 16-bit add (`emit_fsr_pair_add_mem16`), after the
   chain and any static offset.
   **Narrowed (epic-cc#471, 2026-09-19):** FSR0 is now seeded once per
   *access* (one `Inst::Load`/`Inst::Store`) and walked with `POSTINC0`
   across that access's own bytes, since those bytes are emitted by a
   single loop, consecutive and ascending by construction, with nothing
   emitted between them that could touch FSR0. Per-byte re-setup still
   applies *across* separate accesses (no auto-increment carries state
   from one `Inst::Load`/`Inst::Store` to the next) -- see the "Rejected
   alternatives" note below for the boundary this draws.
   **Superseded in part (epic-cc#469, 2026-09-19):** a dynamic term
   whose scale is big enough that a shift-add chain beats its unrolled
   adds is scaled as `scale×idx` from a zero-seeded `LFSR 0, 0x000`
   (doublings plus conditional adds), with the static part re-joining
   as a literal add afterwards. The one-FSR rule and the
   no-auto-increment-across-accesses boundary still hold.
   **Correction (epic-cc#469 review, 2026-09-19):** see item 3's first
   "Superseded in part" note above -- the chain's zero seed cannot
   carry a `SlotValue` origin's runtime pointer value the way
   `Absolute`'s compile-time `base_addr` does; fixed by folding the
   pointer's own two bytes onto the chain-scaled pair with a dedicated
   runtime 16-bit add (`emit_fsr_pair_add_mem16`) after the chain.
   **Narrowed further (epic-cc#472, 2026-09-19):** re-setup *across*
   separate accesses through the same base is no longer always a full
   `LFSR`/base-reload. `Gen.fsr0_holds` tracks `(origin, offset)` --
   what `emit_fsr0_dynamic`/`emit_fsr0_indirect_slot` last grounded FSR0
   in, and at what compile-time offset -- and a later access with `terms
   = []` against the *same* origin and a `target_off >= offset` emits
   only the forward delta (`MOVLW`/`ADDWF`/`ADDWFC`, 4 words, or nothing
   at all when the delta is zero) instead of the full setup. This is not
   auto-increment: it is still one explicit, self-contained add computed
   from tracked compile-time state, the same shape item 3's per-term
   `ADDWF`/`ADDWFC` already uses, not a new addressing mode. `fsr0_holds`
   clears at every label and every `CALL` (mirrors `bsr`'s exact
   invalidation surface: a branch target or a callee's own FSR0 use can
   leave anything there) and additionally whenever this codegen writes to
   a tracked `SlotValue` origin's own slot (`emit_copy_byte`/
   `emit_move_val_to_slot` both check this on every write) -- the one
   straight-line-code case that can change the pointer value a later
   access's reuse check would otherwise wrongly trust, since a pointer
   local not promoted to a pure SSA register is reassigned by an ordinary
   store to that same slot. Backward offsets (`target_off < offset`) fall
   back to the full setup rather than adding a `SUBWF` path, since forward
   struct-field/array-element access is the overwhelmingly common shape
   and the conservative default costs nothing but a missed optimization.
   **Superseded in part (epic-cc#469, 2026-09-19):** a dynamic term
   whose scale is big enough that a shift-add chain beats its unrolled
   adds is scaled as `scale×idx` from a zero-seeded `LFSR 0, 0x000`
   (doublings plus conditional adds), with the static part re-joining
   as a literal add afterwards. The from-scratch per-byte re-setup and
   the one-FSR rule still hold.
   **Correction (epic-cc#469 review, 2026-09-19):** see item 3's first
   "Superseded in part" note above -- the chain's zero seed cannot
   carry a `SlotValue` origin's runtime pointer value the way
   `Absolute`'s compile-time `base_addr` does; fixed by folding the
   pointer's own two bytes onto the chain-scaled pair with a dedicated
   runtime 16-bit add (`emit_fsr_pair_add_mem16`) after the chain.
4. **No `PLUSWn` for dynamic-offset writes.** `PLUSWn` computes its
   effective address from `FSRn + W` at execution time; a write needs `W`
   to hold the byte being stored, colliding with using `W` as the offset.
   All dynamic accesses physically advance `FSRnL`/`FSRnH` and go through
   plain `INDFn`, for reads and writes alike.
   **Narrowed (epic-cc#665):** the collision holds only when the value
   travels in `W`. A byte-indexed access into a RAM global of at most
   128 bytes reads through `PLUSW0` (`MOVF idx,W` + `MOVF PLUSW0,W`),
   and a register-valued store moves through it (`MOVF idx,W` +
   `MOVFF value,PLUSW0`), which preserves `W` and never collides. The
   128-byte ceiling is the signed `W` offset: a valid index stays below
   it, where signed and unsigned address arithmetic agree, and an
   out-of-bounds index is C UB. The rule is safe under either offset
   reading, so no hardware gamble rides on it. `FSR0` stays resident
   across consecutive such accesses (a `PLUSW0` access never moves it)
   under the existing `(origin, offset)` tracking and invalidation.
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
   **Superseded in part (epic-cc#442, 2026-09-17):** the one-term limit
   is lifted. `add_term_to_fsr0`/`add_term_to_fsr1` now loop over every
   entry in `terms`, not just the first, mirroring PIC14 isel's
   `emit_accum_terms`: each term's `MOVF %reg,W; ADDWF FSRnL,F; MOVLW 0;
   ADDWFC FSRnH,F` sequence is a self-contained 16-bit add-with-carry
   against the running `FSRnL`/`FSRnH` value, so terms compose
   additively regardless of order or count (a doubly-indexed pointer
   expression, e.g. `&arr[i][j]`, now compiles instead of panicking).
   This was a real gap, not a deliberate non-goal: single-FSR
   per-byte re-setup (item 3) and no-`PLUSWn` (item 4) are unaffected,
   and no second FSR was needed since this is multiple terms feeding
   one FSR0 setup, not two simultaneously indirect pointers (the
   "Revisit if" section's actual FSR1 trigger, still open). The other
   boundaries in this item still hold.

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
  **Narrowed (epic-cc#665):** reads are no longer the concern (item 4),
  and `MOVFF`-through-`PLUSW0` stores keep the offset in `W`, so the
  uniform ban is lifted for byte-indexed small-array accesses only.
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
  **Narrowed (epic-cc#471):** the objection holds for auto-increment
  *across* separate `Inst::Load`/`Inst::Store` accesses (an implicit
  ordering dependency between them would be exactly the hazard this
  rejected). It does not hold *within* one access's own byte loop, which
  is a single, self-contained emission with a fixed, compiler-known
  byte order and nothing else touching FSR0 in between -- `POSTINC0` is
  now used there (item 3).
- **Porting PIC14's `fsr_window`/window half of `object_span`.** Dead
  machinery on PIC18's flat address space.

## Revisit if

A P4+ fixture needs two simultaneously indirect pointers (add FSR1).

The per-byte re-setup showed up in profiling (epic-cc#469/#471, 2026-09-19)
and was addressed: auto-increment within one access, with an explicit
ordering contract (single-loop, consecutive, ascending, nothing else
touching FSR0 in between).

The re-seeding-across-separate-accesses cost also showed up in profiling
(epic-cc#469/#472, 2026-09-19) and was addressed: a tracked `(origin,
offset)` state lets a same-base access reuse FSR0 via a forward delta
add instead of a full reload, invalidated at labels, `CALL`s, and writes
  to a tracked slot's own address (item 3). `PLUSWn` remains unused (item
  4's write-collision reasoning is untouched by this) and no second FSR was
  introduced.
  **Extended (epic-cc#665):** `PLUSW0` now serves byte-indexed accesses
  into RAM globals of at most 128 bytes (item 4), reusing the same
  resident-pointer tracking across consecutive accesses.

A third profiling finding (epic-cc#469/#473, 2026-09-19) showed call
sites forwarding a pointer value as a plain-ptr/sret argument round-tripping
it through FSR0 for no reason: the argument was already a finished 2-byte
address sitting in a slot, not something needing FSR0's address-computation
machinery at all. Addressed by recognizing that degenerate case in
`emit_call_args` and copying the two bytes directly (item 2). This is
unrelated to FSR0's own setup/reuse machinery (items 3-4): it simply
avoids invoking FSR0 where no dereference is happening.
