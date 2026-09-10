# ADR-033: freestanding malloc/free over a caller-registered arena (epic-cc#343)

**Status:** Accepted 2026-09-10 (implemented in `feat/343-pic18-malloc`)

## Context

docs/35 section 3 lists `malloc / heap` as PIC18 library work, and the
corpus probe named `malloc` was a raw pointer round trip through fixed
SRAM, not a heap. SDCC's pic16 port ships `malloc`/`free` over a
caller-provided arena (`<malloc.h>`: the app defines the heap, calls
`_initHeap`, then allocates), so the honest differential needs a real
epic-cc allocator behind the same call shapes.

## Decision

Ship `malloc`/`free`/`_initHeap` in the driver libc (`stdlib_c.rs`,
gated on including `<stdlib.h>` or `<malloc.h>` like the string/stdio
units), plus `<malloc.h>` with SDCC's `_MALLOC_SPEC` defined empty
(epic-cc has no address spaces):

- First-fit over the registered arena with out-of-band `u8` metadata
  (8 start/len/used entries, bump high-water mark). No splitting or
  coalescing: an 8-bit PIC heap stays auditable.
- `NULL` is the sole out-of-memory signal: `malloc` before any
  `_initHeap` fails, `malloc(0)` yields a 1-byte block, oversize
  requests fail, and `free` on an unknown pointer (including `NULL`,
  which matches no block) is a no-op.
- `free` routes each candidate address through a `volatile` slot:
  PIC18 lowers `icmp` only on slot-held values and a folded GEP has
  no slot. The store/reload materializes the address like a call
  return does. Drop it when `icmp` accepts folded operands.
- One shared-backend extension: `iselcore::resolve_pointers` seeds a
  call result used in pointer position (GEP base, load/store address)
  as an indirect slot. Those LLVM operands are always pointer-typed,
  and both backends already copy retval bytes into the dst slot, so
  this is the established load-pointer model (epic-cc#183), extended
  to call results. Previously such programs panicked; no working
  program changes shape.
- The Tier-2 probe (PIC18-only) registers a 64-byte arena, allocates
  two blocks, writes both, frees one, allocates again, and folds the
  observable bytes to 0x3B. PIC18-only because SDCC's pic14 port has
  no `<malloc.h`, so no shared source can exercise `malloc` there;
  the libc itself is conformance-pinned on all three cores by a
  driver e2e test.

Provenance: designed from first principles (first-fit free lists are
textbook); SDCC's sources were never read. Only its published
interface was consulted: the `malloc.h` declaration lines, the
app-defines-`heap` plus `_initHeap` usage its own regression test
shows, and the manual-level fact that pic14 has no `<malloc.h>`.
Nothing was translated, so there is no provenance debt; the
out-of-band table is structurally unlike SDCC's in-band headers
in any case.

## Rationale

- The arena model mirrors the oracle, so one probe source runs under
  both compilers with no `#ifdef` (the corpus contract bans
  compiler-conditional sources).
- Out-of-band integer metadata keeps every loop an index loop and
  every return a single GEP, the shapes both backends lower; the two
  workarounds (volatile probe, call-result seed) are each documented
  at the site with their removal condition.
- `noinline` on all three entry points keeps each under the 877A
  page limit, the same reason the string unit uses it.

## Rejected alternatives

- **In-band headers (SDCC's shape).** Self-referential structs with
  pointer fields need pointer materialization PIC18 lacks; the table
  keeps metadata in integers.
- **Bump-only allocator.** `free` must reclaim (the probe frees and
  re-allocates); a leak-only heap is not `free`.
- **Ignoring the `_initHeap` buffer.** A compat shim that accepts and
  ignores the arena wastes RAM and lies about ownership; adopting it
  is barely more code.
- **An IR `ptr` flag on call returns.** Twelve constructor sites plus
  canonical-text changes, versus a fifteen-line use-driven seed at
  the one site that needs it.
- **All-core Tier-2 probe.** Impossible without `#ifdef`: p18 needs
  `<malloc.h>`, pic14 only has `<stdlib.h>` malloc with different
  setup. The e2e test covers pic14-family conformance instead.

## Revisit if

- A pic14-family `malloc` ticket uses that port's `<stdlib.h>`
  spelling for its own probe (verified present during #343).
- Fragmentation matters enough to want splitting or coalescing.
- `icmp` learns folded operands (remove the volatile probe) or a
  second consumer needs call-result pointers in select arms (extend
  the seed past GEP/load/store positions).
