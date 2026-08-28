# ADR-026 -- PIC18 struct ABI: XC8 byte alignment

**Status:** Accepted 2026-08-28<br>
**Decides:** `epic-cc#166`<br>
**Parent:** `docs/29-pic18-port-design.md` (§7, the datalayout-proxy assumption this revises)

## Decision

For PIC18 devices the driver adds `-fpack-struct` to the pinned clang
invocation. Every C record then lays out with member alignment 1, which is
XC8's PIC18 struct ABI: a mixed 8/16-bit struct keeps its natural wire size
(`configuration_descriptor` 9 bytes, not 10). irparse reconstructs the
layout from the packed struct types clang prints in the `.ll` text
(`%struct.X = type <{ ... }>`): fields at running offsets, no padding,
struct alignment 1. No flag threads past the front end; every downstream
stage keeps consuming the folded byte sizes and offsets it already gets.

PIC14 is deliberately unchanged: it keeps the msp430 natural alignment
(i16 align 2). XC8 byte-aligns structs on PIC14 too, but switching the
settled backend's ABI is a separate decision with its own verification
pass, not a rider on the PIC18 fix.

## Rationale

* **m-stack cannot be fixed any other way.** Its `usb_ch9.h` takes the
  `__XC8` branch, which applies no packing at all: XC8's byte alignment
  comes from the ABI, not from pragmas. The static wire-size checks
  (`STATIC_SIZE_CHECK_EQUAL(sizeof(struct configuration_descriptor), 9)`)
  compile only when the unpacked struct is 9 bytes. Patching the vendored
  tree to add packing is exactly what epic-hal#89's rules forbid.
* **The consistency argument is airtight because every layout-derived
  constant comes from exactly one of two places, both packed-aware.** clang
  bakes `sizeof` folds, alloca sizes and memcpy lengths from its record
  layout; irparse recomputes gep byte offsets, global sizes and initializer
  bytes from the type table. clang prints `<{ ... }>` precisely when its
  record layout packs, and LLVM packed-struct semantics (running offsets,
  alignment 1) equal the `-fpack-struct` layout, so the two agree by
  construction. Probe-verified on the pinned clang 20.1.8: sizeof 9,
  offsetof(w) 2, `alignof` 1, nested packed-outer/unpacked-inner shapes
  included.
* **`-fpack-struct` changes only record layout.** Every other property the
  msp430 proxy provides (8-bit char, 16-bit int/pointers/long, type widths)
  is untouched, so the blast radius is exactly the gap the issue names.

## Alternatives rejected

* **A second proxy triple (`-target avr`) for PIC18.** AVR also aligns
  everything to 1, but it changes plain `char` to unsigned by default and
  `double` to 32 bits: ABI shifts well beyond struct layout, for a
  placeholder target nobody audits. The msp430 proxy stays.
* **`#pragma pack` support only.** Even with irparse honoring packed
  types, m-stack still does not pack under `__XC8`, so the reported build
  stays broken without vendored-tree edits; and every user struct without a
  pragma keeps the wrong size versus XC8.
* **irparse-only align-1 (no clang flag).** clang would keep baking
  packed-incorrect `sizeof`/alloca/memcpy constants while irparse laid out
  packed: the two sides disagree and the difference is a silent miscompile.

## Known trade-offs

* **PIC14 mixed structs still diverge from XC8** (10 vs 9 bytes for the
  descriptor above). PIC14 programs compiled only by epic-cc are unaffected
  (layout is self-consistent); a differential XC8 comparison on
  struct-heavy PIC14 code needs the follow-up that flips PIC14 too.
* **A TU mixing `-fpack-struct` with user `#pragma pack(N)`, N > 1** can
  print a shape irparse models as fully packed while clang's record layout
  rounds some member to N. XC8 has no meaningful `#pragma pack(N)` story
  and no known consumer uses the combination; if one appears, the decode
  width asserts are the tripwire (an initializer that decodes to the wrong
  byte count fails loudly).

## Revisit if

* A PIC14 consumer needs XC8 wire-format structs (flip the flag for
  `Core::Pic14` behind the same verification stack).
* The front end gains a real PIC18 TargetInfo (clang built from source
  already), making the proxy triple obsolete; the irparse packed-struct
  reader stays correct regardless of which target produces the text.
