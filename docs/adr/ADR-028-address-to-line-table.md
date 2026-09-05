# ADR-028 -- Address-to-line table (thread SrcLoc to asm)

**Status:** Accepted 2026-09-05<br>
**Decides:** `epic-cc#238` (debugger phase 1)<br>
**Parent:** `docs/32-debugger-design.md` §4 (phase plan); umbrella `#203`

## Decision

* Every `ir::Inst` carries an `Option<SrcLoc>` (a `loc` field on each struct
  variant, plus one `Inst::loc()` accessor). `irparse` already resolves
  `!dbg`/`DILocation` per line; it now stores the location on every
  instruction it constructs, not just `Call`.
* The canonical IR text (`ir::serialize`/`parse`) does **not** carry the
  location. The line table is built in the normal compile path, which never
  reparses canonical text; `--emit ir` is a secondary path. Deferred to a
  future format-change PR if a later phase needs reparse.
* `isel`/`isel-pic18` emit a parallel per-line `Vec<Option<SrcLoc>>`
  (`select_with_locs`), index-aligned with the asm text. Compiler-generated
  lines (header, `__start`, const tables, prologue glue) get `None`.
* `schedule`/`banking`/`peephole` thread the parallel vector through their
  reorders/insertions/drops (`schedule_with_locs`, `assign_banks_with_locs`,
  `optimize_with_locs`). Preservation contract:
  * an inserted `BANKSEL` inherits the nearest preceding original
    instruction's line (the banked operand it precedes);
  * a peephole-elided `MOVLW k; MOVWF PCLATH` pair drops its two locs with it;
  * a schedule reorder moves the loc with the line.
* The driver emits an address-to-line artifact via a new `--line-table <file>`
  flag (mirrors `--map`), built by walking the final asm text with the same
  pass-1 semantics `asm::assemble` uses (tracking `org`, labels, `.align`,
  `.table`, `end`), pairing each word address with the parallel locs vector.
  Format: `; epic-cc line table for <device>` header, then one
  `file:line:col 0xNNNN` record per word, sorted by address, compiler-generated
  words omitted.

## Rationale

The debugger needs an address-to-line table, and the raw material already
existed: `-gline-tables-only` is passed to clang, `irparse` resolves
`!dbg`/`DILocation` into `ir::SrcLoc`, and `SrcLoc` was threaded through
`legalize`/`callgraph`/`alloc` for diagnostics. But it stopped there: `SrcLoc`
was attached only to `Call.loc`, and no backend stage read it. The work was
carrying an attribute that already existed on the IR the rest of the way to a
final address.

The parallel-vector approach keeps every existing text boundary byte-identical
until the driver opts in with `--line-table`: the gpasm oracle, golden
fixtures, and the entire existing suite are untouched. Attaching the line to
the asm text itself (a trailing `; loc` comment) was rejected: it would change
the diffable `.asm` boundary, break dozens of multi-line `asm.contains` asserts,
and require correctness-sensitive comment-stripping in `schedule`'s
classification and `peephole`'s PCLATH-elision soundness.

## Alternatives rejected

* **Attach the line to the asm text as a comment.** Changes the diffable
  `.asm` boundary and breaks the gpasm oracle and golden fixtures; rejected
  above.
* **Build the table inside `asm::assemble`.** `asm` is a leaf crate that takes
  only text; threading a parallel vector into it couples the assembler to the
  debugger. Re-walking the final text (as `isel::verify_page_fit` already does)
  keeps `asm` unchanged and the line table a driver-level artifact.
* **Inherit the nearest preceding call's line for non-call instructions.**
  Loses accuracy (a `store` on line 12 between two calls on lines 10 and 20
  would get line 10) and is exactly the "silent wrong line" failure the
  debugger must not have. The per-instruction line is already parsed; dropping
  it is the bug, not the design.

## Revisit if

* A later phase needs the line to survive a canonical-text reparse (e.g. a
  `--check`-style round-trip). Then `ir::serialize`/`parse` must carry the
  location, a format change to a load-bearing boundary.
* A new backend stage inserts, merges, or drops instructions without updating
  its parallel locs vector. The preservation contract must be extended to it.
