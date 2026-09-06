# Debugger phase 2: typed variable table

**Ticket body (to become `epic-cc#NNN`)**. Mirror `#246`'s
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

- **Full `-g` in the front end, and consume the debug records it
  brings.** Switching `-gline-tables-only` to full `-g` is *not* a bare
  flag flip. With the pinned clang 20.1.8 at `-O1` (the toolchain's
  fixed `-O1`; AGENTS.md), full `-g` emits LLVM **debug-record lines**
  like `#dbg_value(i16 %0, !18, !DIExpression(), !23)`, and at `-O0`
  `#dbg_declare(ptr %1, !19, ...)`, in the function body (observed
  2026-09-06 via `/opt/clang/bin/clang -g -S -emit-llvm`). The parser
  has no filter for `#`-prefixed lines: they route through `parse_ll`'s
  body loop (not empty/`;`/label/`switch`) into `parse_inst`, whose
  opcode dispatch (`crates/irparse/src/lib.rs:3303`, `other =>
  panic!("SPIKE LIMIT: unsupported opcode ...")`) panics on
  `#dbg_value`. So phase 2 must teach **irparse to parse and drop
  `#dbg_value`/`#dbg_declare`/`#dbg_assign` record lines** (a `#`-line
  guard in the `parse_ll` body loop, plus a `#dbg_` drop in `parse_inst`
  for safety), emitting no `Inst`, alongside the flag flip. Full `-g`
  also adds `!llvm.dbg.cu` metadata (already skipped as a `!` line) and
  a `, !dbg !N` tail on global/`define` lines (tolerated today; keep as
  a known-dropped-token case). `-g` is the only clang-flag depth change
  the debugger needs; its cost is this debug-record handling, not a
  larger front-end contract.
- **The C-name to allocation-key bridge.** `DILocalVariable`'s name is
  the C identifier (`buf`); `AllocLayout.local`s (`crates/alloc`) are
  keyed `{func}::{name}` from the liveness frame's **SSA def names**
  (`format!("{}::{name}", f.name)` at `alloc:1265`), which for promoted
  or unnamed temporaries are numbers, not C names (`local main::1`,
  `local main::2` in the map e2e). The `#dbg_value`/`#dbg_declare`
  record's value operand is precisely the C-name to SSA-value link:
  irparse records each `DILocalVariable`'s SSA value/address operand
  from the debug record, and this phase's join is **C-name -> recorded
  SSA key -> `AllocLayout` address** (`AllocLayout.globals` for globals,
  keyed by name, are already address-keyed by C name). A variable
  optimized away or otherwise unmapped to an allocation has no address
  and is omitted from the table (defined fallback, not an error).
- **The typed table and its artifact.** The deliverable that phase 4
  consumes is the **in-process type table** built by joining
  `DILocalVariable`/`DIType` metadata against allocations, exposed so
  phase 4's sidecar encoder can walk it (names, scopes, addresses,
  full type trees). A **text artifact** (mirroring `--line-table`,
  e.g. `--var-table <file>`, `global <name> 0xNN TYPE` /
  `local {func}::{name} 0xNN TYPE`) is emitted **for human inspection
  only**; it is not the encoder input, and its TYPE field is the
  minimal debug print of a type (`char`, `int`, `long`, `enum tag`,
  `struct tag`, `T*`, `T[N]`), deliberately flat because it carries
  no struct member offsets or array bounds. The in-process table is
  the single source of truth for phase 4's DWARF.

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

- Full `-g` is passed and existing fixtures with locals compile and
  produce HEX without an irparse panic (the `#dbg_value`/`#dbg_declare`
  record drop works). The unit test pinning `BASE_ARGS`
  (`crates/driver/src/clang.rs:192`) is updated with the `-g` flag.
- irparse tests: a `!DILocalVariable`/`!DIType`-bearing `.ll` parses
  into the extended table with correct name/scope/type, and `#dbg_*`
  record lines emit no `Inst`.
- Join tests: a program with global + local vars of all covered types
  resolves each C name to the address `AllocLayout` assigns, through
  the recorded SSA-key bridge; an optimized-away variable is omitted,
  not an error.
- The `--var-table` artifact renders one flattened record per mapped
  variable with the correct address and TYPE string.
- No regression in the line table: `--line-table` output identical to
  phase 1's for existing fixtures (the `-g` switch and dbg-intrinsic
  elision must not disturb `SrcLoc` resolution).
- Existing test suite stays green.

Non-goal: no ELF/DWARF encoding here. The in-process table is phase
4's input; the text artifact is inspection only.

## Notes for the reader

This is a **compiler front-end/data phase only**; nothing runs a
program yet. Phase 2's `#dbg_*` record consumption is the load-bearing
risk: full `-g` is only valuable if irparse absorbs the debug-record
lines clang emits as metadata, not as instructions, so the compiler
output is unchanged. The design doc records the flag depth as "the one
clang-flag change the whole debugger needs"; this phase is where that
change's cost is paid, in the irparse record drop, not in a larger
front-end contract.
