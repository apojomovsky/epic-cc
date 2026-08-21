# ADR-017: CC-4 inline assembly (naked, module asm, opaque statement blocks)

**Status:** Accepted 2026-08-21 (implemented in feat/cc4-inline-assembly)

## Decision

Implement rungs 1 to 3 of the D-3 ladder, exactly as `docs/31` defines, and defer rung 4:

* **Rung 1 - naked functions and `module asm`.** A function marked `EPIC_NAKED` (`__attribute__((naked))` in the header) is a real function: `callgraph` counts it toward the 8-deep hardware stack and `alloc` overlays its (normally empty) frame. Its body is emitted verbatim. File-scope `asm("...")` becomes LLVM `module asm "..."` and is emitted verbatim at the top of the program. No raw `.asm` compile-unit inputs in v1.
* **Rung 2 - intrinsics.** Header-only macros expanding to a single opaque `asm volatile("...")`: `__epic_nop`, `__epic_clrwdt`, `__epic_sleep`, `__epic_di`, `__epic_ei`. They travel the same `Inst::Asm` path as rung 3, no special lowering.
* **Rung 3 - opaque statement-level `asm volatile("...")` with no operands.** Ordered against volatile accesses by clang. Panics for any operand form (including `"*m"` memory operands, deferred to rung 4) and for register constraints (`"=r"`).

IR surface: `Module.module_asm: Vec<String>`, `Func.naked: bool`, `Inst::Asm { template: String, clobbers_memory: bool }`. Text form: `module_asm "..."` lines before globals; `fn foo() [naked] ()` or `fn foo() [isr] [naked] ()`; `asm "..." [memory]`.

Front end: `irparse` lifts `module asm`, the `naked` attribute (inline and `attributes #N`), and `call void asm sideeffect` with unescaping (`\"`, `\\`, `\0A` to `\n`, `\XX` hex) and constraint checks. A naked function may not contain non-`Asm` insts and its trailing `unreachable` is dropped. `sanitize_symbols` leaves asm string content untouched.

Pipeline: `callgraph` ignores `Asm` (no edges). `alloc::def_width(Asm)=None`, so a naked frame overlays normally and stays empty. `isel` and `isel-pic18` emit module asm at the top and verbatim `Asm` blocks bracketed by `; --- asm start ---` / `; --- asm end ---` markers. A naked body is label plus verbatim lines, no prologue or synthetic return. Inline blocks are verbatim at their block position. `word_size` counts each non-empty non-comment verbatim line as one word, so `verify_page_fit` remains correct. Every block is treated as clobbering `W`, `STATUS`, bank/IRP and `FSR`.

Banking/peephole: `banking::assign_banks` never inserts `BANKSEL` inside a marker bracket and leaves `tracked = UNKNOWN` on entry and exit, so the next banked operand gets a full `BANKSEL`. `is_bank0_only` returns false when asm is present. `peephole` splits on the markers and optimizes segments independently.

Assembler: `asm` accepts `label: instruction` on one line and `INTCON` is now an `equ` (`0x0B` on PIC14, `0xFF2` on PIC18), so module and inline asm needing that name assemble under both `gpasm` and the internal assembler.

Driver: `epic-cc.h` adds `EPIC_NAKED` and the five intrinsics. Any `*.asm`/`*.s` input is rejected early with `epic-cc: .asm inputs are not supported in this build; use EPIC_NAKED functions`.

Errors (panics-are-the-error-surface): `asm with operands is not supported in this build (rung 4 deferred)`, `asm: register constraints are not supported on PIC`.

## Rationale

D-3's order is preserved because the cheapest rungs cover the most real usage: whole routines via naked, single instructions via intrinsics, and the `bcf/bsf INTCON` guard via opaque blocks. Naked is chosen over a file-scope blob precisely because the blob is opaque to `callgraph` and `alloc`, forcing pinned scratch for its whole lifetime, which is the `epic-math` limitation D-3 aims to remove. Module asm is retained because `crates/asm` already exists and it gives a free-form top-of-file escape hatch for directives.

Header-only intrinsics keep one lowering path. A dedicated `Inst::Intrinsic` would add a second isel entry for no user-visible gain until an intrinsic needs optimization understanding.

Conservative clobbering is permanent, not a v1 simplification: clang on `msp430` validates clobber names and only `"memory"` and `"cc"` pass, so PIC registers cannot be named. Assuming `W`, `STATUS`, bank unknown after every block is the only sound choice. The markers make the barrier explicit for `banking` and `peephole` without teaching them to parse arbitrary assembly.

Rejecting operands in v1 is a deliberate shortening of the contract. The `"*m"` memory form requires address-map substitution and an allocation-aware lowering that touches `alloc`, `isel`, and the overlay story. D-3 marks it speculative and the call was not to build it until rungs 1 to 3 prove insufficient in practice, which matches the HAL's actual need (the benchmark argues `epic-math` should use C, not hand asm).

## Rejected alternatives

* **Driver-level text splicing (no IR node).** Cheaper but loses callgraph and alloc visibility and breaks banking barrier reasoning. Rejected for the naked-vs-blob reason above.
* **Intrinsics-only.** Does not cover the `bcf/bsf INTCON` idiom or any user hand sequence. Rejected as insufficient for a standalone toolchain.
* **Implementing rung 4 now (`%0`/`"+m"` substitution).** More plumbing for a need that has no HAL consumer in the CC-4 window. Deferred per D-3.
* **Raw `.asm` compile-unit inputs.** Same opacity cost as the file-scope blob, plus driver plumbing, for an escape hatch naked already provides. Deferred.

## Revisit if

* A real HAL consumer needs to name a C local inside an asm block, then build rung 4 (address-map substitution, one `*m` per operand, GEP-derived pointers panic).
* A measured hot path shows the conservative `W`/`STATUS`/bank clobber is the bottleneck, then add out-of-band `; epic:clobber=...` inside the template, parsed and stripped, as D-3 notes.
* `__epic_di`/`__epic_ei` need distinct PIC18 encodings beyond `INTCON`, then make the header device-aware via a driver-injected `-D__EPIC_PIC18__`.
