# Debugger phase 2: typed variable table

**Ticket body (to become `epic-cc#NNN`)** — mirror `#246`'s
Summary / Scope / Acceptance structure.

---

## Summary

Second phase of the source-level debugger (`docs/34-debugger-design.md`
§4; umbrella `#203`). Phase 1 (`#238`) carried a `SrcLoc` on every
`ir::Inst` and shipped the `--line-table` artifact. This phase adds the
**typed variable table** that phase 4's gdbstub reads to answer gdb's
`print <name>` (typed, via DWARF) and `info address <name>`. Scope for
v1 (settled in brainstorming 2026-09-06): **scalars + aggregates** C
types. PIC14 only, proving the pattern per `docs/34` §5.

Depends on phase 1 (landed). Blocked by: none. Umbrella: `#203`.

## Scope

- **Full `-g` in the front end.** `crates/driver/src/clang.rs` passes
  `-gline-tables-only` (lines 33, 192). Phase 2 switches to full `-g`
  to surface `DILocalVariable` and `DIType` nodes in the `.ll` text.
  This is the one place the `-g` depth changes the input format, and
  the design doc records it as the single front-end-flag change the
  whole debugger needs: `-gline-tables-only` deliberately omits
  variable and type metadata.
- **Extend `irparse`'s metadata table.** `build_debug_info`
  (`crates/irparse/src/lib.rs:608`) already classifies
  `!DIFile/!DILocation/!DISubprogram/!DILexicalBlock` into a
  `DebugInfoTable`. Phase 2 adds classification of
  `!DILocalVariable` and `!DIType` (`!DIBasicType`, `!DICompositeType`
  with `!DIDerivedType` members), and of `!DILocalVariable`'s
  `scope`/`line`. The existing pattern (two-pass decode, id-keyed
  table) extends directly.
- **Join against allocation.** `AllocLayout` (`crates/alloc`) already
  has the address half: `AllocLayout.globals` (name -> addr) and
  `.locals` (keyed `{func}::{name}`, addr). Phase 2 joins variable
  identity to addresses via the existing key. A type carries a name and
  a scope; the join resolves it to a `{func}::{name}` key that the
  existing address table already carries.
- **Emissions for phase 4 consumption.** The phase-4 sidecar (ELF+DWARF)
  is built in that phase. Phase 2's deliverable is the in-process type
  table, exposed so phase 4 can walk it, plus a text artifact
  (mirroring `--line-table`) for inspection and for the phase-4
  sidecar encoder as its input. Text format is one `local {func}::{name}
  0xNN TYPE` / `global {name} 0xNN TYPE` record per variable, TYPE being
  the minimal debug string for the C type (`char`, `int`, `long`,
  `enum tag`, `struct tag`, `T*`, `T[N]`, `T`).

### Type coverage (v1, scalars + aggregates)

- `char`, `signed char`, `unsigned char`
- `int`/`unsigned int`, `long`/`unsigned long` (16-bit and 32-bit
  forms epic-cc supports)
- pointers `T*`
- `enum` (underlying integer type)
- `struct`/`union` (members with byte offsets), arrays `T[N]`

Explicit **non-goals** (deferred): bit-fields, `_Bool`, C99
flexible array members, typedefs as distinct DWARF wrappers (resolved
to their base type), and any location form beyond a compile-time
constant address (impossible on the no-stack PIC14, see `docs/34` §1).

## Acceptance

- `irparse` tests: a `!DILocalVariable`/`!DIType`-bearing `.ll`
  parses into the extended table with correct name/scope/type.
- Join tests: a program with global + local vars of all covered types
  resolves each name to the address `AllocLayout` assigns.
- The text artifact (driver flag) renders one record per variable with
  the correct address and a correct TYPE string, for a fixture
  exercising every covered type shape.
- No regression in the line table: `--line-table` output identical to
  phase 1's for existing fixtures (the `-g` switch must not disturb
  `SrcLoc` resolution).
- Existing test suite stays green.

Non-goal: no ELF/DWARF encoding here. Phase 4 consumes this table.

## Notes for the reader

This is a **compiler front-end/data phase only**; nothing runs a
program yet. The consumer is phase 4's gdbstub. The reason the
`-g` flag depth matters (and only here) is that clang is
out-of-process, epic-cc never runs LLVM's backend where DWARF
emission lives, and `-gline-tables-only` strips the very
`DILocalVariable`/`DIType` nodes this phase needs.
