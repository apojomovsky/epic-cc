# ADR-018: CC-2 freestanding libc subset (stdint, stdbool, stddef, string; stdlib joined 2026-08)

**Status:** Accepted 2026-08-21 (implemented in `feat/cc2-libc`)

## Context

`epic-hal` includes `stdint.h`, `string.h`, `stdbool.h` and `stddef.h`
throughout (`docs/31`). The driver invokes clang with `-ffreestanding
-nostdinc`, so the host's headers are deliberately invisible and
`-isystem` does not help. CC-2 must ship those four headers and, for
`string.h`, real code that links and runs on both `p16f877a` and
`p18f4550`. It blocks HAL-3.

`string.h` is the only one of the four that needs code.

## Decision

Ship the four headers as `pub const` strings in `crates/driver/src/`
(`stdint_h.rs`, `stdbool_h.rs`, `stddef_h.rs`, `string_h.rs`), the shape
`epic_cc_h.rs` already uses. Content is minimal C99 freestanding, with
`size_t` and `ptrdiff_t` matching the msp430 datalayout proxy (16-bit).

2026-08: `stdlib.h` joins the set as a header-only addition (`stdlib_h.rs`,
`size_t` only), because vendored third-party sources such as m-stack include
it for `size_t` alone (epic-cc#163). It stays header-only until a consumer
needs code.

Ship the `string.h` implementation as a freestanding C translation unit
(`string_c.rs`). The driver writes the headers into the temp include
directory next to `epic-cc.h` and, when any input line contains both
`#include` and `string.h`, compiles `__epic_string.c` with the same clang
flags and hands it to `llvm-link` as one more unit. Gating on the include
keeps the code out of programs that never use it and leaves the symbols
free for a user's own implementation, so no `weak` linkage is needed.

**These entry points ship:** `memcpy`, `memmove`, `memset`, `memcmp`,
`strlen`, `strnlen`, `strcpy`, `strncpy`, `strcat`, `strncat`, `strcmp`,
`strncmp`. The header declares exactly what exists, so a missing function
is a clang error at the call site rather than a link-time surprise.

The implementation is written to stay inside the pointer shapes both
backends lower, which is a real constraint on the C, not an accident:

* **Index loops, not pointer walks.** `d[i] = s[i]` keeps the
  loop-carried value an integer phi; `*d++ = *s++` would make it a `phi
  ptr`, which nothing resolves.
* **Backward loops decrement first** (`while (i > 0) { i--; ... }`).
  Writing `d[i - 1]` makes clang emit a GEP with offset `-1`, and
  `ir::Gep::k` is a `u8`.
* **`strcat`/`strncat` carry one running index**, not `dest[d + i]`,
  which folds to a two-term GEP that PIC18 does not lower.

`memchr`, `strchr`, `strrchr` and `strstr` are **deferred**: each returns
a pointer into the middle of its argument, which needs pointer-value
materialization that PIC18 lacks entirely and PIC14 only half has.

### IR and backend changes this required

`ir::Param` gains `ptr: bool` (text form `<name>=ptr`). Width alone
cannot identify a pointer parameter: an `i16` is also two bytes, and
keying off the width instead made `resolve_pointers` treat every 16-bit
scalar parameter as a pointer base, which the differential fuzzer caught
as two miscompiles. `iselcore::resolve_pointers` seeds a plain pointer
param off that flag.

Both backends read such a slot the way they already read an `sret` slot,
through the existing indirect FSR helpers, rather than through a
parallel code path. A `byval` param stays direct: its slot *is* the
object.

`irparse` accepts the shapes clang emits for pointer code: `ptr` as a
16-bit type, `null`/`zeroinitializer`/`undef` as `0`, and the `returned`
parameter attribute. The unknown-token panic in `parse_param` stays; new
attributes are allow-listed one at a time so an unmodeled one still
aborts loudly.

## Consequences

The headers unblock HAL-3. The conditional extra unit keeps `string`
code out of builds that do not use it.

Fixing CC-2 surfaced three latent miscompiles that were not
string-specific and are corrected here (see the commit history):

1. `irparse` labelled every function's implicit entry block `"0"`, but
   LLVM numbers unnamed values in one sequence, so an N-parameter
   function's entry block is `%N`. Phi copies are keyed on the
   `(pred, block)` edge, so the entry edge's copies were silently
   dropped and any loop in a function with parameters started its
   counter from whatever the overlaid slot held. It only bit once a
   sibling call dirtied that slot.
2. `isel`'s GEP pointer-value path conflated the constant GEP offset
   with which byte of the pointer it was emitting, so byte 1 of a
   zero-offset pointer got a carry propagation that both added a phantom
   1 and destroyed the carry an enclosing 16-bit compare needed.
   `memmove`'s `d > s` therefore always answered false.
3. New code emitted file-register operands as `0x{:03X}` while
   `banking` rewrites them by matching the `{:02X}` form, so bank
   rewriting silently did not happen for addresses above `0x7F`.

## Rejected alternatives

* **Unconditional string unit.** Adds code even when unused and risks a
  duplicate symbol against a user's own `memcpy`.
* **Relaxing the assembler's `f <= 0x7F` assert to accept any address.**
  Masking an address to seven bits without knowing the bank is exactly
  the silent miscompile that assert exists to catch. The real defect was
  the operand format mismatch above.
* **Special-casing pointer params inside each backend's address
  resolution.** Tried and reverted: it duplicated the indirect-slot
  helpers badly, and being keyed on "is a param" rather than "is a
  pointer" it also caught `byval` params, whose slot is the object.
* **Headers as real files under `vendor/`.** `pub const` keeps the
  driver self-contained and inside the git-tracked set the Docker build
  copies, like `epic-cc.h`.

## Revisit if

A consumer needs `memchr`/`strchr`/`strrchr`/`strstr`. Those need a
pointer value derived from a GEP to be materialized into a slot or a
return register: PIC18 needs the path added, and PIC14's needs widening
past the pointer-param base it accepts today. Both currently panic
loudly rather than miscompile.
