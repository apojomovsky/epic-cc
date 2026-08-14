# 04 — Compiler pipeline design

> **Approval status:** Section 1 (pipeline and repository shape) was presented to the user
> and is awaiting explicit approval — the user redirected to a research survey and this
> documentation phase before answering. Sections 2–4 (the allocator/banking core;
> verification harness; phasing) were **outlined but never presented in full**.
> Do not treat this document as signed off. See
> [`08-status-and-next-steps.md`](08-status-and-next-steps.md).

## Ten stages, each a crate, each with a text boundary

| # | Stage | In → Out |
|---|---|---|
| 1 | `driver` | `.c` files → invokes clang → `.ll` |
| 2 | `irparse` | `.ll` text → our IR |
| 3 | `wholeprog` | N modules → one merged module, externs resolved |
| 4 | `legalize` | i16/i32/float ops → i8 sequences + runtime calls |
| 5 | `callgraph` | merged IR → call graph, recursion check, ISR trees, HW-stack depth check |
| 6 | `alloc` | call graph + locals → static addresses across 4 banks |
| 7 | `isel` | IR → PIC14 instructions (virtual, pre-banking) |
| 8 | `banking` | dataflow-minimised `BANKSEL` / `PAGESEL` insertion |
| 9 | `peephole` | pattern-driven cleanup |
| 10 | `asm` | instructions → `.hex` + `.lst` + `.map` |

### Why every boundary is text

Every arrow is a snapshottable, diffable artifact. When the agent hits a miscompile, it
bisects **which stage** broke it before reading any code. This is the difference between an
agent that makes progress overnight and one that thrashes, and it is a direct consequence
of the autonomy requirement in [`00-charter.md`](00-charter.md).

## Front-end invocation details

**Datalayout proxy: `-target msp430`.** We are not generating MSP430 code. We want clang to
make the right ABI-independent type decisions, and MSP430's datalayout is a near-perfect
match for PIC14:

| Type | MSP430 | What PIC14 / XC8 wants |
|---|---|---|
| `char` | 8 | 8 |
| `int` | 16 | 16 |
| pointer | 16 | 16 |
| alignment | byte | byte |

**Open question:** whether to use MSP430's datalayout wholesale or declare our own. Since
we only consume IR (never LLVM's codegen), we are free to specify a custom datalayout
string. To be settled during the spike.

**Optimization pipeline:** a *curated* pass selection, not `-O2` wholesale. Some LLVM
optimizations are actively harmful here — SROA creating many i16 values increases RAM
pressure on a machine with 368 bytes, and the optimizer normalizes shifts-and-adds into
multiplies which we must then re-expand (a problem llvm-mos flagged explicitly). mem2reg is
still valuable for SSA construction.

## The hard core (stages 5–8)

This is where essentially all the difficulty lives. Detail in
[`01-target-pic14.md`](01-target-pic14.md); algorithms in [`02-prior-art.md`](02-prior-art.md) §5.

### Stage 5 — call graph

- Build the whole-program call graph
- **Detect recursion → compile error.** There is no addressable stack to fall back to.
  Note llvm-pic's recorded advice from the llvm-mos team: *"More difficult to not have
  recursion in LLVM than to have recursion"* — a problem we sidestep by owning our IR.
- Identify interrupt roots (`__interrupt` functions) as a **separate call tree**
- Check maximum call depth against the **8-level hardware stack**, accounting for interrupt
  nesting

### Stage 6 — overlay allocation

The core of the compiler. Functions that cannot be simultaneously live share RAM addresses.
Structurally this is a graph-colouring problem over an interference graph derived from the
call graph — Muchnick ch. 16 (register allocation) and ch. 19 (interprocedural analysis)
are the implementation references.

Allocation targets, in preference order:
1. **Common RAM 0x70–0x7F** (16 bytes) — imaginary registers, no BANKSEL cost
2. Banked GPR — placed to minimise bank switching

Functions reachable from **both** the main tree and an interrupt tree cannot share storage
with themselves; they must be duplicated or given non-overlapping frames.

### Stage 7 — instruction selection

Single-accumulator machine, 35 instructions. Technique: **burg-style tree-pattern
matching**, as presented in Fraser & Hanson's lcc book (§13–18) with complete working code
generators.

Special consideration: `DECFSZ`, `INCFSZ`, `BTFSC`, `BTFSS` are *skip* instructions that
conditionally skip the next instruction. This is the core's only conditional control flow
and shapes how compare-and-branch lowers.

### Stage 8 — banking and paging

Two related dataflow passes:

- **BANKSEL minimisation** — NP-hard; 2-approximation published (CASES'06). Shaped like
  partial redundancy elimination, so Muchnick ch. 8 and ch. 13 supply the machinery.
- **PAGESEL / PCLATH minimisation** — published heuristic (arXiv 1008.0909).

## Device description

`devices/pic16f877a.toml` — memory map, bank layout, SFR names/addresses/bit fields, config
words, hardware stack depth. See ADR-004. Later devices generated from gputils `.inc` files.

## Explicitly not in v1

- **Separate compilation** — whole-program is the point
- **Debugger / COFF / ELF output** — HEX, listing, and map only

## Harvard / `const` data — unresolved

`const` tables must live in program memory, via `RETLW` tables or flash self-read. LLVM IR
assumes one flat address space. Planned approach: an address-space attribute plus a
dedicated lowering pass.

**llvm-mos provides no prior art here** — the 6502 is not Harvard. This is spike question 4
and the least-derisked part of the design.
