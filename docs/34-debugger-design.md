# 34: source-level debugger design

> **Approval status:** front end (gdbstub/RSP) and v1 target scope
> (simulator only, no hardware) were decided by the user during
> brainstorming on 2026-09-05. Phase 1 (below) is landed: `epic-cc#238`,
> merged as `#240`, decided in
> [`ADR-028`](adr/ADR-028-address-to-line-table.md). Phases 2-4 are still
> proposal, pending their own tickets and review; this document was
> originally numbered 32 and renumbered to 34 after two unrelated docs
> claimed that range while it sat as a local draft.

**Goal:** a source-level debugger for programs compiled by epic-cc, comparable
in spirit to gdb, so a user can set a breakpoint on a C line, step, and print
a variable by name while running the program under `crates/sim`. Real
hardware (ICSP debug executive, PICkit/ICD-class tools) is an explicit
non-goal of this document; it is a separate, materially harder proposal for
later.

No prior design work exists for this: `docs/` and the ADR index carry no
mention of a debugger, DWARF, or gdb before this document. The board does
carry one thing: #203, an unscoped idea issue filed to give this a home
in conversation, which independently landed on the same simulator-first
framing this document formalizes ("epic-cc's own simulator... a much
smaller lift, and something XC8 fundamentally can't offer"). This document
supersedes it as the scoped version; #203 stays open as the tracking
issue for the whole effort.

---

## 1. Why this is more tractable than it looks

`crates/alloc`'s doc comment is the load-bearing fact: PIC14 has no stack.
Every local is statically allocated and overlaid across the whole-program
call graph (`base(f) = max over callers of the caller's frame end`). Every
variable epic-cc ever compiles therefore has a **compile-time-constant
address**, never a frame-relative location. This removes the part of a
normal debugger's design that is usually hardest: stack unwinding, CFI,
frame-base computation, register-relative DWARF location expressions
(`DW_OP_fbreg`, `DW_OP_call_frame_cfa`). Every location this project ever
needs to express is `DW_OP_addr(constant)`, the simplest form DWARF has.

The driver already produces half of the variable-debug-info problem for
free: `--map` (`crates/driver/src/report.rs:map_text`) emits `global <name>
0xNN` / `local {func}::{name} 0xNN` today, for exactly this reason.

**Line-table parsing is also already built, further than expected going
in.** `crates/driver/src/clang.rs` already passes `-gline-tables-only`, and
`irparse` already resolves `!dbg`/`DILocation`/`DISubprogram` into
`ir::SrcLoc` (`crates/ir/src/lib.rs:141`) attached to instructions
(`crates/irparse/tests/debug_loc.rs` covers this in detail). But it exists
only to print `file.c:line:col` in a backend panic message, not as debug
info: `SrcLoc` usage stops at `legalize`/`callgraph`/`alloc` (all three
reference it for diagnostics) and never reaches `isel`, `banking`,
`peephole`, or `asm`: grepping those four crates for it turns up nothing.
So Phase 1 (below) is narrower than a clean-slate read suggests: no new
clang flag, no new parsing. The work is carrying an attribute that already
exists on `ir::Inst` the rest of the way to an address, through the four
stages that currently have no obligation to preserve it. Phase 2's move to
full `-g` (for `DILocalVariable`/`DIType`) is the only place a clang-flag
change is still needed, since `-gline-tables-only` deliberately omits
variable and type metadata.

## 2. What clang can and cannot give us

Clang is an out-of-process front end here (`-S -emit-llvm`, text in); epic-cc
never runs LLVM's backend (`AsmPrinter`/`MC`), which is where DWARF section
emission actually lives. So there is no way to inherit clang's DWARF output
directly: epic-cc has no object-file backend for it to plug into, and
building an ELF-object backend just to carry sections through the ten-stage
pipeline is not something this project needs for its shipped output (Intel
HEX has no such capacity, and doesn't need to grow one).

What clang *does* give us, at zero extra front-end cost, is `-g` debug
metadata (`!dbg`, `DILocation`, `DILocalVariable`, `DIType`) attached
directly to instructions in the `.ll` text it already emits. `irparse`
already parses that text; teaching it to also read this metadata is how
epic-cc gets accurate source lines and C types without writing any of its
own front-end line-tracking. This is the one real "don't build it
ourselves" opportunity on the compiler side.

## 3. Confirmed: gdb requires a client-side ELF+DWARF symbol file

Spiked 2026-09-05, empirically, before writing this document. Setup: a
throwaway statically-linked x86-64 ELF built with `gcc -g -O0` (a **known**
gdb architecture, chosen deliberately to isolate this question from the
separate "will gdb accept an unknown PIC14 target description" question),
and a from-scratch Python program answering only raw RSP packets (`?`, `g`,
`m`, `Z0`, `qSupported`, ...) with fabricated values it controls.

Findings:

- `break t.c:9` and `break helper` resolved to concrete addresses
  (`0x401888`, `0x401865`) purely from the ELF's `.debug_line`/symtab. No
  address-based packet crossed the wire to produce that resolution.
- With the stub returning fabricated bytes 7 and 9 for two globals,
  `print in` → `$1 = 7 '\a'`, `print out` → `$2 = 9 '\t'`: gdb issued a real
  `m<addr>,1` read over RSP and applied the DWARF `unsigned char` type to
  whatever came back over the wire, not the ELF's static (zero-initialized)
  value.
- A fabricated `rip` in the `g` packet reply made gdb print `helper () at
  t.c:5` unprompted, purely from a raw address matched against
  `.debug_line`.

**Conclusion.** RSP itself carries no symbols, types, or line numbers: it
is a strictly address/register-level protocol. All source-level behavior
(`break file:line`, typed `print`, `info line`) is resolved by gdb itself,
client-side, from a symbol file loaded via `file`/`symbol-file`. A gdbstub
target with no accompanying symbol file gets only raw address/register
debugging (`break *0x1234`, `x/4xb 0x70`), not acceptable for what this
project wants. So epic-cc unavoidably needs to emit a small **ELF+DWARF
sidecar artifact**, alongside the HEX, purely for gdb's consumption. It is
never flashed and does not touch the existing HEX output path.

This is a smaller lift than it sounds: `gimli::write` (pure Rust, no LLVM
dependency; the tool Cranelift/wasmtime use for exactly this "non-LLVM
backend, still needs DWARF" situation) encodes the DWARF; the `object`
crate writes the ELF container generically. Given every location is
`DW_OP_addr(constant)` (section 1), the DWARF this project emits is close
to the simplest valid form the format supports: one compile unit, flat
`DW_TAG_subprogram`/`DW_TAG_variable` entries, `DW_TAG_base_type` for the
handful of C primitive types epic-cc supports, and a `.debug_line` program
built directly from the line table Phase 1 (below) produces.

**Open risk, not yet spiked.** PIC14 is not a gdb-known architecture. The
spike above used x86-64 specifically to control for that variable. gdb does
support fully custom targets via a `qXfer:features:read` target-description
XML (established pattern: several architectures shipped this way before
gaining built-in gdb support), and DWARF processing itself is
architecture-agnostic in gdb, but the exact interaction between a
synthetic/placeholder `e_machine` in the sidecar ELF and a custom register
target description needs its own short spike before Phase 4 is designed in
detail. Flagged here rather than assumed solved.

## 4. Phase plan

**Phase 1: line table. Landed** (`epic-cc#238` / `#240` /
[`ADR-028`](adr/ADR-028-address-to-line-table.md)). Every `ir::Inst` now
carries an `Option<SrcLoc>`; `isel`/`isel-pic18` emit a parallel per-line
`Vec<Option<SrcLoc>>` index-aligned with the asm text, and
`schedule`/`banking`/`peephole` thread that vector through their
reorders, insertions and drops under an explicit preservation contract
(an inserted `BANKSEL` inherits the nearest preceding original
instruction's line; a peephole-elided pair drops its locs with it; a
schedule reorder moves the loc with the line). The canonical IR text
does not carry the location, deliberately: the normal compile path never
reparses it, and `--emit ir` is a secondary path. The driver emits the
artifact via a new `--line-table <file>` flag, `file:line:col 0xNNNN`
per word, compiler-generated words omitted. The doc-comment note above
(no clang or `irparse` change needed) held; see the ADR for what was
actually rejected along the way, notably attaching the line to the asm
text as a trailing comment, which would have changed the diffable
`.asm` boundary and broken the gpasm oracle.

**Phase 2: typed variable table.** Extract `DILocalVariable`/`DIType` from
the same `-g` metadata. Join against `AllocLayout.globals`/`.locals`
(`crates/alloc`), which already has the address half of this for free.

**Phase 3: sim control surface.** `crates/sim` runs to completion or a
cycle count today; it has no halt/resume/breakpoint state machine. Add:
run-until-address, register read (`W`, `PC`, `STATUS`, `FSR`, active bank),
memory read/write, and a step primitive at instruction granularity (line
stepping rounds up from this in the adapter, not in `sim` itself).

**Phase 4: gdbstub adapter.** New crate implementing `gdbstub::Target` over
Phase 3's control surface. Consumes Phases 1/2's data to emit the ELF+DWARF
sidecar (section 3) at build time. Ships as a new binary or driver
subcommand, e.g. `epic-cc-gdbserver <hex> <sidecar.elf> --port <port>`, used
as `gdb <sidecar.elf> -ex "target remote :<port>"`. The open risk in
section 3 (custom architecture target description) needs resolving before
this phase's design is finalized.

## 5. Non-goals for this document

- Real hardware debugging (ICSP + Microchip's debug-executive protocol,
  PICkit/ICD-class tools). Materially different problem, largely
  undocumented outside Microchip's own tooling; a separate proposal if ever
  pursued.
- PIC18. Land the PIC14 path and prove the pattern first.
- Any variable-location form beyond constant addresses (frame-relative
  locations, register-allocated locals), not needed given section 1, and
  not planned.
