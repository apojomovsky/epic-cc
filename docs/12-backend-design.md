# 12 — Backend design (approved)

**Status:** Approved 2026-08-14. Sections presented and user-approved.
This is the consolidated spec for the whole-program PIC14 backend; the implementation plan
derives from it. It supersedes the scattered design notes in
[`04-pipeline-design.md`](04-pipeline-design.md), [`05-verification.md`](05-verification.md),
and the status doc, which remain as the detailed references.

## 1. Architecture

Approach A (ADR-001): clang runs out-of-process and emits LLVM IR **as text** (`.ll`); we
parse it and implement our own whole-program PIC14 backend. We never link libLLVM and never
touch SelectionDAG, GlobalISel, TableGen, or MCTargetDesc.

The ten-stage pipeline and repository shape are **approved** ([`04-pipeline-design.md`](04-pipeline-design.md)):

| # | Stage | In → Out |
|---|---|---|
| 1 | `driver` | `.c` → invokes clang → `.ll` |
| 2 | `irparse` | `.ll` text → our IR |
| 3 | `wholeprog` | N modules → one merged module |
| 4 | `legalize` | i16/i32/float ops → i8 sequences + runtime calls |
| 5 | `callgraph` | merged IR → call graph, recursion check, ISR trees, stack-depth check |
| 6 | `alloc` | call graph + locals → static addresses across 4 banks |
| 7 | `isel` | IR → PIC14 instructions (virtual, pre-banking) |
| 8 | `banking` | dataflow-minimised `BANKSEL`/`PAGESEL` insertion |
| 9 | `peephole` | pattern-driven cleanup |
| 10 | `asm` | instructions → `.hex` + `.lst` + `.map` |

Every stage boundary is a **diffable text artifact** (snapshottable, bisectable). Each stage
is its own crate. We own the assembler/linker down to Intel HEX (ADR-002); `gpasm` is a
test-time oracle only, never a build dependency.

Front end: `-target msp430` datalayout proxy (`p:16:16`, byte alignment, native 8/16-bit),
with a **curated** optimization pass list at `-O1`/`-O2` — not `-Oz`, which emits
arbitrary-width integers (`i17`) and intrinsics.

## 2. The allocator / banking core (stages 5–8)

This is where essentially all difficulty lives. The target has **no addressable stack**, so
the allocator is the compiler.

### Two layers

- **Call-graph overlay (frame level).** Every local is statically allocated; functions that
  cannot be simultaneously live (per the call graph) share RAM.
- **Value allocation (within the live set).** Values get addresses in preference order:
  **common RAM 0x70–0x7F first** (no `BANKSEL`), then banked GPR minimising switches.
  Liveness-based reuse (interference-graph colouring) is **first-version**, not an
  optimization: the spike measured 26 bytes of demand against 16 bytes of common RAM on an
  eleven-line program. Allocation is **function-scoped** — slots are keyed by
  `(function, value)`, never bare SSA name.

### Stage 5 — call graph

Build the whole-program call graph; **recursion is a compile error** (no escape hatch for
v1); identify `__interrupt` roots as a separate call tree; check maximum call depth against
the 8-level hardware stack, including interrupt nesting.

### Stage 6 — overlay allocation

Graph colouring over an interference graph derived from the call graph (Muchnick ch. 16,
19).

**Interrupt/main shared-function policy: duplicate.** A function reachable from both the
main and interrupt trees is **emitted twice** (once per tree) with independent frames.
Rationale: 8K words of flash against 368 bytes of RAM — spending flash to save RAM is the
correct trade, and it is the simpler correct approach. (Decision: user, 2026-08-14.)

### Stage 7 — instruction selection

Single accumulator, 35 instructions. Technique: **burg-style tree-pattern matching** (lcc,
Fraser & Hanson, has complete working generators). Spike-validated shapes: `select`→skip,
16-bit carry chains through `W`, `FSR`/`INDF` indirect access, `RETLW` const reads.

### Stage 8 — banking and paging

- **`BANKSEL` minimization** — NP-hard even with fixed bank assignment; published
  2-approximation (CASES'06), shaped like partial redundancy elimination.
- **`PAGESEL`/`PCLATH` minimization** — published heuristic (arXiv 1008.0909).

### Pointer / `const` split (from the spike)

LLVM IR has one flat address space; the target has two. The backend classifies globals by
the `constant` marker: RAM globals lower via `FSR`/`INDF`; flash (`constant`) globals lower
via `RETLW` lookup tables — a `CALL` into a computed-jump table, with a `PCLATH` +
page-crossing story. This split is first-class, not an afterthought. See
[`11-pointer-const-findings.md`](11-pointer-const-findings.md).

## 3. Verification harness

Built **first**, before the compiler, so the oracle exists before the thing it judges. The
goal is a closed loop that manufactures its own minimal failing test cases with no human in
the sequence.

- **Our own PIC14 simulator** (~1500 lines, 35 instructions): deterministic, fast,
  embeddable in `cargo test`, cycle-counting, asserts on internal state. Removes a GPL
  process boundary from the inner loop.
- **Oracles:** XC8 (black-box differential testing — same source, both outputs on our
  simulator, compare state; never reverse-engineered, ADR-006) and gpsim (independent
  semantic reference to validate our simulator).
- **`gpasm` cross-check** — assemble our `.asm`, diff HEX against our own assembler,
  isolating "assembler wrong" from "codegen wrong."
- **YARPGen** (UB-free) over Csmith for random generation, constrained to our subset and
  the 877A's limits.
- **C-Reduce/cvise** for automatic reduction; the load-bearing skill is writing oracle
  scripts that distinguish "still miscompiles" from "now fails for another reason."
- **Stage-boundary snapshots** (`insta`) pin each text stage boundary.
- **Hardware-in-the-loop** as the final acceptance test (out of scope for automation).

## 4. Phasing and milestones

Sequenced so hard, high-risk parts come early and large-but-decoupled parts come last:

1. **Verification harness first** — simulator, XC8 differential runner, `gpasm`
   cross-check, snapshot infrastructure.
2. **Integer spine** — core C89, 8/16-bit ints, control flow, non-recursive calls. Overlay
   allocation + `BANKSEL`/`PAGESEL` minimisation land here.
3. **Pointers, arrays, structs** — the `FSR`/`INDF` codegen (de-risked; the single-pointer
   ISA is the quality ceiling).
4. **Interrupts + SFR headers + device description** — duplicated shared functions.
5. **32-bit `long`** + soft mul/div/mod runtime.
6. **Random testing at scale** — YARPGen + cvise loop, unsupervised.
7. **Soft-float** — largest library chunk, least coupled to the hard backend.

## 5. Decisions recorded

| Decision | Status |
|---|---|
| Ten-stage pipeline + repository shape | ✅ approved |
| Rust (ADR-005) | ✅ approved |
| Own assembler/linker to HEX (ADR-002) | ✅ approved |
| clang out-of-process front end (ADR-001) | ✅ approved |
| Nix flake + direnv, clang pinned (ADR-007) | ✅ approved |
| Duplicate interrupt/main shared functions | ✅ approved (2026-08-14) |
| Recursion = compile error, no escape hatch for v1 | ✅ approved (2026-08-14) |

## 6. Open items (do not block the plan, but must be resolved before hard-coding)

- **`[VERIFY]` items** in [`01-target-pic14.md`](01-target-pic14.md): memory map, bank
  ranges, common RAM extent (0x70–0x7F, 16 bytes), flash size (8K words), `const`-in-flash
  mechanism — confirm against DS39582 and DS33023.
- **Datalayout:** MSP430's is the working default; a custom one is possible but not required.
- **Clang pass list:** curated at `-O1`/`-O2`; exact list measured during the integer spine.
- **Legalizer generality:** how general the widening/narrowing story must be (the `i17`
  problem from `-Oz`).
