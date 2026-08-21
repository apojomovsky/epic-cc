# ADR-011: Multi-translation-unit front end (llvm-link merge, sanitize in irparse)

**Status:** Accepted 2026-08-20 (implemented in feat/multi-tu-front-end)

## Decision

The driver (CC-1, ecosystem integration D-7) compiles N `.c` files by running
clang once per file, merging the resulting `.ll` files with `llvm-link -S`,
and handing the single merged `.ll` to the existing single-module pipeline
unchanged:

1. **`llvm-link`, out of process, does the merge.** Same posture as clang
   itself: text in, text out, nothing links libLLVM. `llvm-link` ships
   beside clang in both the dev image and the release bundle (6 MB against
   clang's 218 MB), located via `clang_discovery::resolve_llvm_link`.
2. **`wholeprog` validates what `llvm-link` lets through, it does not
   link.** `llvm-link` leaves an unsatisfied `declare` in place rather than
   failing, which downstream would become a `CALL` to a label the
   assembler never heard of. `wholeprog::merge` now asserts exactly one
   `main` and that every `Inst::Call` target is defined, using only the
   `Inst` enum already in the IR, no format change.
3. **Symbol sanitization is a text transform in `irparse`, not a walk over
   the parsed IR, in `wholeprog`.** `llvm-link` renames colliding internal
   symbols by appending a dot and a number (`@helper` / `@helper.3`); the
   `gpasm` byte-for-byte oracle rejects dots in identifiers.
   `irparse::sanitize_symbols(ll: &str) -> String` rewrites them to
   underscores before `parse_ll` ever runs, skipping text inside `"`
   quotes (a C string constant can itself contain `@`) and leaving LLVM
   intrinsics (`@llvm.memcpy...`) untouched.
4. **A conventional CLI replaces the two-positional-argument form.**
   `epic-cc [options] <input.c>... -o <file> --device <name>`, plus
   `-I`/`-D` forwarded to clang, `--emit ll|ir|asm|hex` to stop at any
   stage boundary, `--save-temps`, `-v`. `--device` is required; it
   retires the driver's hard-coded `PIC16F877A`.

## Rationale

- **Merging translation units is not concatenation.** It requires
  same-name-different-symbol renaming, one-definition selection across
  a `declare`/`define` pair, and dropping resolved declarations.
  `llvm-link` was probed against the pinned clang 20.1.8 with two units
  carrying identical `static` names and a cross-unit global (2026-08-20):
  it renamed one of each colliding pair (`@scratch` / `@scratch.4`,
  `@helper` / `@helper.3`), kept one definition of the shared global, and
  dropped the satisfied `declare`. Reimplementing this by hand risks
  getting one-definition or linkage rules subtly wrong, which is a
  miscompile, the one failure class this project's architecture exists to
  prevent.
- **Sanitizing in `irparse` rather than `wholeprog` is a deliberate
  deviation from the original design note in
  [`31-ecosystem-integration-design.md`](../31-ecosystem-integration-design.md)
  D-7, which said "symbols are sanitized once, in `wholeprog`".** A name
  reaches the parsed IR through seven different fields
  (`Func.name`, `Global.name`, `Call.func`, `Val::Global`,
  `GepBase::Global`, `Load.ptr`, `Store.ptr`) spread across `ir::Inst`'s
  twenty variants. An IR-level walk would have to stay exhaustive across
  all of them forever; one text pass over `@`-prefixed identifiers in the
  merged `.ll`, before `parse_ll` runs, covers every one of them in a
  single pass that cannot silently miss a future `Inst` variant.
- **`llvm-link` runs even for a single input file.** Verified against
  `crates/driver/tests/fixtures/add.c`: the only differences from running
  clang alone are the module-ID comment, `source_filename`, and metadata
  ordering, all of which `irparse` already ignores or never reads. Running
  it unconditionally keeps one code path instead of a single-file
  fast-path plus a multi-file path that only gets exercised by some
  callers.
- **`--emit` exposes the pipeline's existing text boundaries rather than
  adding new ones.** The `.ll` and `.asm` stages were always diffable
  text internally; `--emit` makes that a documented CLI feature instead
  of something only visible by reading test helpers like
  `array_e2e.rs::array_layout()`.

## Rejected alternatives

- **Implement the merge in `wholeprog` by hand.** Requires teaching
  `irparse` to preserve LLVM linkage (`internal` is currently stripped as
  a noise attribute), adding declaration state to `ir::Func` and
  `ir::Global`, and writing renaming plus one-definition resolution.
  Full ownership of subtle semantics that ship in the box, for the
  reward of not depending on a 6 MB binary already present in the
  toolchain.
- **Textual concatenation of the `.ll` files with ad hoc mangling.**
  Cheaper, and wrong for any program with two same-named `static`s in
  different files, which is a realistic shape (see the acceptance
  fixtures `multi_tu_a.c`/`multi_tu_b.c`, both declaring `static
  scratch`/`static bump`).

## Revisit if

A future clang version stops preserving the section-name/metadata
equivalence `llvm-link` was probed against, or a fixture surfaces a
`.ll` construct `sanitize_symbols` does not anticipate (its collision
check panics loudly in that case rather than silently merging two
distinct symbols, so this should surface as a build failure, not a
miscompile, if it ever happens).
