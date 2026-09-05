# Phase 1: address-to-line table (thread SrcLoc to asm)

**Status:** Design for `epic-cc#238` (debugger phase 1)
**Parent:** `docs/32-debugger-design.md` §4 (phase plan); umbrella `#203`

## Problem

The debugger needs an address-to-line table: for every word of the final
program, which C source line produced it. The raw material already exists:
`-gline-tables-only` is passed to clang, `irparse` resolves `!dbg`/`DILocation`
into `ir::SrcLoc`, and `SrcLoc` is threaded through `legalize`/`callgraph`/
`alloc` for diagnostics. But it stops there: no backend stage reads it, and
`isel`/`banking`/`peephole`/`asm` have no obligation to preserve it.

The ticket's premise is "no clang flag change and no new parsing; thread
`SrcLoc` from where it stops today (`alloc`) through `isel`, `banking`,
`peephole` to `asm`." Investigation shows that premise is **not quite right**:
`SrcLoc` is attached only to `Call.loc`, not to every `Inst`. This changes the
shape of the work.

## Evidence (what the code actually does)

- `ir::SrcLoc` (`crates/ir/src/lib.rs:143-152`): `{ file: String, line: u32, col: u32 }`.
- `SrcLoc` is attached **only** to `Inst::Call` as `Call.loc: Option<SrcLoc>`
  (`crates/ir/src/lib.rs:180`). No other `Inst` variant carries a location.
- `irparse` computes `cur = dbg_loc(line, dbg)` for **every** instruction line
  (`crates/irparse/src/lib.rs:2644`), but only `Call` stores it
  (`loc: cur.clone()`, line 2843). Every other variant drops it.
- `legalize` writes `loc: None` on every synthesized call (runtime routines,
  float/shift helpers) and never reads `.loc`.
- `callgraph` reads `Call.loc` only to build `edge_locs` for recursion panics.
- `alloc` never touches `SrcLoc`.
- `isel` (`crates/isel/src/lib.rs:select`, 6396) emits flat `Vec<String>` asm
  text; `Gen` has no loc field; `emit_inst(&Inst)` has the `Inst` (and thus
  `Call.loc`) in hand but discards it.
- `banking`/`peephole`/`schedule` operate on flat text lines (`&str`), no
  structured instruction type, no metadata.
- `asm::assemble` (`crates/asm/src/lib.rs:11`) assigns the word address: pass 1
  walks lines tracking `org`, pass 2 encodes into `Vec<u16>` indexed by word
  address. The word index IS the per-instruction address.
- The diffable-text-boundary convention: `--map` (`crates/driver/src/report.rs:
  map_text`) and `alloc::map_text`/`callgraph::edges_text` are deterministic,
  sorted, `;`-comment-header text artifacts surfaced via CLI flags.

## Decision

Two coupled decisions: (A) where the line is captured, and (B) how it survives
the text-based stages to a final address.

### A. Capture the line on every `Inst`, not just `Call`

The ticket says "no irparse change," but the evidence shows `SrcLoc` is only on
`Call`. To build a line table for the whole program we need a line for every
instruction, and the only place that line exists is irparse's per-line
`dbg_loc`. So:

- Add `pub loc: Option<SrcLoc>` to the `Inst` enum as a **wrapper field**:
  `Inst { kind: InstKind, loc: Option<SrcLoc> }` — or, less invasively, add a
  `loc` field to each struct variant. The wrapper is preferred: one field, one
  place to thread, and `Inst` is already a tagged union so the wrapper is the
  natural shape.

  **Alternative rejected:** keep `SrcLoc` only on `Call` and synthesize lines
  for non-call instructions by inheriting the nearest preceding call's line.
  This loses accuracy (a `store` on line 12 between two calls on lines 10 and
  20 would get line 10) and is exactly the "silent wrong line" failure the
  debugger must not have. The per-instruction line is already parsed; dropping
  it is the bug, not the design.

- `irparse` already computes `cur` per line; store it on every `Inst` it
  constructs. This is a mechanical change: every `Inst::X(X { ... })` gains
  `loc: cur.clone()`.

- **Canonical text round-trip:** `ir::serialize`/`parse` currently drop `loc`
  (`Call.loc` is documented "the canonical text does not carry it"). For the
  line table to survive `--emit ir` reparse and the `--check`-style round-trip,
  the canonical text must carry the line. Add a `; loc file:line:col` comment
  (or a `loc` token) to each serialized instruction line, and parse it back.
  This is a format change to the canonical text; it is the diffable-text
  boundary, so it is the right place for the line to live.

  **Risk:** the canonical text is a load-bearing input format (like the `.ll`
  surface). Changing it means every `ir::serialize`/`parse` round-trip test and
  any golden `.ir` fixtures must be updated. This is a one-time cost, not a
  recurring one.

### B. Carry the line through the text stages as a parallel vector

From `isel` onward the stages are text-based (`Vec<String>`/`&str`), with no
structured instruction type. Threading `SrcLoc` through them means carrying a
**parallel per-line metadata vector** alongside the text, updated by each pass
that inserts, merges, or drops lines. This is the preservation contract the
ticket asks for.

Concretely:

- **isel** emits `Vec<String>` lines. Change `Gen::emit` to also push a
  `Option<SrcLoc>` into a parallel `Vec<Option<SrcLoc>>`, sourced from the
  `Inst` being emitted. `emit_inst(&Inst)` already has the `Inst`; thread its
  `.loc` through. Instructions isel synthesizes (prologue, `__start`, const
  init, runtime routines) get `None` (compiler-generated, no source line).

- **schedule** (identity today, but a real reorder in a follow-up): carries the
  parallel vector; a reorder moves the metadata with the line. Phase 1 is
  identity, so this is a pass-through.

- **banking** inserts `BANKSEL` lines. **Contract:** an inserted instruction
  inherits the nearest preceding original instruction's line (the ticket's
  proposed default). Since banking processes lines in order and inserts before
  a banked operand, the inserted `BSF/BCF STATUS` gets the same line as the
  instruction it precedes. This is the natural rule and matches the ticket.

- **peephole** (PCLATH elision) drops `MOVLW k; MOVWF PCLATH` pairs. **Contract:**
  a dropped pair's line is dropped with it (no line to attach to a removed
  instruction). A combine keeps the earlier of the two lines it merges (the
  ticket's proposed default). Phase 1's only pass is a drop, so the combine rule
  is future-proofing.

- **asm** assigns the final word address. The address-to-line table is built
  here: walk the final text with the same pass-1 semantics as `asm::assemble`
  (tracking `org`, labels, `.align`, `.table`), pairing each word address with
  the parallel metadata's line. This mirrors `isel::verify_page_fit`'s existing
  re-walk of the final text.

  **Alternative rejected:** build the table inside `asm::assemble` itself.
  `asm` is a leaf crate that takes only text; threading a parallel vector into
  it couples the assembler to the debugger. Re-walking the final text (as
  `verify_page_fit` already does) keeps `asm` unchanged and the line table a
  driver-level artifact.

### Artifact and surfacing

- New driver flag `--line-table <file>` (mirrors `--map`), written after
  `asm::assemble_words` in `crates/driver/src/main.rs` (after line 384).
- Format, following the diffable-text-boundary convention:
  ```
  ; epic-cc line table for p16f877a
  ; <file>:<line>:<col> <word-address>
  main.c:3:1 0x0000
  main.c:4:1 0x0001
  ...
  ```
  Deterministic, sorted by word address, `;`-comment header. One record per
  line. Compiler-generated words (no source line) are omitted or marked
  `; <generated>` — omitted is cleaner and matches "no line to attach."

## Non-goals (this ticket)

- Variable types / full `-g` (phase 2).
- Any runtime debugging: `crates/sim` control surface (phase 3), gdbstub
  adapter and ELF+DWARF sidecar (phase 4).
- PIC18: the line table is core-agnostic (both backends emit text), but the
  preservation contract and artifact are validated on PIC14 first, per the
  design doc's "land the PIC14 path and prove the pattern first."

## Verification

- `cargo test -p irparse` (canonical-text round-trip with `loc`).
- `cargo test -p isel` / `-p banking` / `-p peephole` (parallel-vector
  preservation; banking's BANKSEL-inherits-preceding-line contract).
- A new `crates/driver` e2e test: compile a small C program with a known
  multi-line body, assert the `--line-table` output maps the expected word
  addresses to the expected source lines, and that a `BANKSEL`-inserting
  program's inserted words carry the preceding line.
- `make ci-local` (full suite) and the takeoff ritual before PR.

## Open questions for approval

1. **Canonical-text change (A):** adding `loc` to the canonical text is a
   format change to a load-bearing boundary. Acceptable? (It is the only way
   the line survives `--emit ir` reparse and round-trip.)
2. **Wrapper vs per-variant field (A):** `Inst { kind, loc }` wrapper vs a
   `loc` field on each struct. Wrapper is preferred (one field, one thread
   point); per-variant is more invasive but keeps each struct self-contained.
3. **Artifact format (B):** the `; file:line:col <addr>` shape above, or a
   different convention?
