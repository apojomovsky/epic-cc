# Runtime-indexed reads through ccp_sel-style pointer selects

**Date:** 2026-08-24
**Parent:** `docs/31-ecosystem-integration-design.md` (HAL-2), epic-hal#71 acceptance audit
**Ticket:** `epic-cc#114`
**Status:** design, awaiting approval

---

## Problem

Runtime indexing into a `const` struct table in flash is unsupported when the
pointer is a *value*, not a folded GEP chain. The CCP driver's pattern
(`ccp_sel()` returns `&addrs[inst]`, callers deref the result plus field
GEPs) produces two IR shapes, both broken:

1. **Inlined select (the real HAL shape, static `ccp_sel` at `-O1`):**

   ```llvm
   %9 = select i1 %8, ptr getelementptr inbounds nuw (i8, ptr @addrs, i16 6), ptr @addrs
   %43 = load i8, ptr %9, align 1
   ```

   `irparse`'s select arm parsing extracts only the `@addrs` global from a
   `getelementptr` arm and **drops the `+6` offset** (silent miscompile, the
   wrong element is read). The select's `dst` then has no `resolve_pointers`
   entry, so `isel` panics with `no gep for pointer %9`.

2. `noinline` `ccp_sel` (call form, the minimal repro):

   ```llvm
   ; main
   %2 = tail call fastcc ptr @ccp_sel(i8 %1)
   %3 = load i8, ptr %2
   %4 = getelementptr inbounds nuw i8, ptr %2, i16 1
   ...
   ; ccp_sel
   %3 = select i1 %2, ptr getelementptr inbounds nuw (i8, ptr @addrs, i16 4), ptr @addrs
   ret ptr %3
   ```

   The pointer-returning call result `%2` and its child GEPs have no resolvable
   base: `iselcore: no gep for pointer %2 (chain base missing, key main::2)`.

The existing `(base, k, terms)` model and the RETLW reader machinery already
handle runtime-indexed reads (`gep @table +0 +1*%i` → `CALL __read_table` with
the index in W). The missing surface is precisely: pointer **values** derived
from `select`, and pointer values **returned from calls**, flowing into
loads/stores/memcpys.

## Scope

1. **irparse: preserve inlined-GEP offsets in select arms.** Materialize a
   select arm that is an inlined `getelementptr` as a fresh `Gep` inst (the
   exact mechanism `parse_call_ptr_val` already uses for call args), so
   `(base, k, terms)` survive. Fixes the silent miscompile independent of
   anything else.

2. **legalize: sink pointer-returning selects into callers.** For a call
   `%r = call @f(...)` where `@f`'s body is exactly
   `%p = select <cond>, <ga>, <gb>; ret ptr %p` (cond a function of its
   params), replace `%r` in the caller with the select, inlining the cond's
   icmp/consts with the actual args substituted. Keep `@f` only if it has
   other callers; a pointer-returning function with any other body shape
   keeps the loud panic at isel. This is a strict, narrow inliner, not a
   general one.

3. > iselcore: fold pointer-typed selects. `resolve_pointers` resolves a
> select whose two arms fold to the same base with identical term sets:
> `select c, base+kA, base+kB` folds to `(base, min(kA,kB), terms +
> (|kA-kB|, c))`. The cond reg is used directly with either arm order:
> the term's scale is the offset difference and the cond's 0/1 polarity
> picks the arm, so no xor/arm-swap normalization is needed (the scale is
> the difference of two u8 offsets, always representable). A const cond
> folds to the selected arm; mismatched bases or term sets panic loudly.
> The folded result feeds the existing reader/FSR lowering unchanged
> (`emit_ptr_load_byte`, `emit_ptr_store_byte`, `memcpy`).

4. **isel: pointer-typed selects emit nothing.** `Inst::Select` whose `dst`
   has a resolved pointer fold emits nothing (exactly like `Inst::Gep`);
   every load/store/memcpy through it lowers via the fold. Value selects
   (`i16 5` vs `i16 6`, the IRQ-id select in the HAL) keep the existing
   if/else copy lowering, distinguishable by whether the arms are pointer
   folds.

## Non-goals

- A general pointer-value ABI: opaque pointer-returning functions whose body
  is not a select of two folded GEPs, pointer `phi`s, pointer arithmetic on
  non-folded values. These keep the existing loud panics; nothing is
  silently miscompiled.
- Generic `inttoptr` SFR stores (the `EPIC_REG8(a->cprl) = ...` stores in the
  CCP driver already lower through the literal-pointer path once `a->cprl`
  resolves; the pre-existing `isel: no address for @addrs` when a const table
  is looked up as a RAM global stays a loud error, fixed by keeping the
  table `const` so it is never RAM-allocated).
- PIC18 `TBLRD` const reads (out of scope for this ticket; the shared
  `iselcore` changes are additive and backward-compatible for `isel-pic18`).
- Removing the `__EPIC_CC__` RAM fallback in epic-hal, tracked there per the
  audit (see Acceptance).

## Acceptance

A driver e2e fixture (`crates/driver/tests/fixtures/const_ccp_addrs.c` +
`const_ccp_addrs_e2e.rs`):
- `static const ccp_addrs_t addrs[2]` stays `const` in source;
- `ccp_sel()` with a **runtime** instance index (a global the sim preloads);
- reads all four fields of both instances through `a->field`, plus a
  `select`-shaped value (i16) regression;
- sim asserts the field bytes for instance 0 and instance 1, and the map
  proves `addrs` has no RAM address (flash only).

Plus isel-level tests for the offset-preserving select parse and the select
fold. `make test` green, `make check-warnings` clean.

## Risks

- **clang IR shape drift.** `-O1` may pick different select arm orders,
  offsets, or fold `ccp_sel` into the caller (removing the call shape). The
  noinline probe pins the call-return shape in the tests; the inlined select
  shape is pinned by the real HAL IR captured above. Both shapes must keep
  passing.
- **Scale overflow / non-foldable arms** (arms with different bases, or
  `|kA-kB|` > 255, or nonzero term sets on both arms): loud panic, never a
  silent fold. The CCP table (8 bytes, offsets 0/4/6) fits.
- **Complemented cond.** A fresh `xor` inst is legal IR; verify the cond is
  an `i1` reg (clang's icmp/select shape always is) and keep the fold's
  emit single-use.
- **HAL unblocking.** With `ccp_sel` handled, the `__EPIC_CC__` RAM fallback
  in both CCP drivers can be removed (epic-hal side); the compile surfaces
  then also hit `&h` byval-arg and `icmp ptr` on the handle (pre-existing,
  separate ticket epic-cc#73 and friends, not regressions).
