# 35: SDCC parity epic

> **Approval status:** design approved by the user on 2026-09-06. This
> document is the design of record for the SDCC parity epic (tracking
> issue, epic-cc). The implementation plans derive from it and do not
> exist yet.

**Goal:** for every target epic-cc supports, be at least as good as
SDCC: same or better language surface, same or better code size, same
or better speed, verified objectively against SDCC's own output. SDCC
is the abandoned incumbent; the bar is "cover every SDCC capability",
never "match SDCC's feature set" (we do not regress to SDCC's
limitations).

**Definition of done (epic-level):** all three family sub-epics green,
and the epic-cc vs SDCC comparison is a CI artifact that fails on
regression. Nothing in this epic disassembles SDCC output; every
comparison is black-box.

---

## 1. Why the bar is breadth, not quality

SDCC's strength is surface breadth, not codegen. Its pic16 (PIC18) port
is "not yet mature and still lacks many features" (SDCC manual 4.10),
unmaintained, and has **no regression gate** (manual 4.10: "no
automatic regression tests are currently performed for the PIC16
target"). Its pic14 port is the same (manual 4.9). So "at least as good
as SDCC" is a low bar on code size and cycles and a real bar on
language surface: C99/C11, 64-bit ints, bit-fields, unions, malloc,
math.h, recursion, priority interrupts.

epic-cc is already ahead of SDCC in two places, and the DoD must not
regress them:

- **Struct/union as function parameter and return value.** SDCC's
  pic14 and pic16 ports cannot do either (manual 3.1.3/3.1.4). epic-cc
  does, via `sret`/`byval`, on both cores.
- **libc.** SDCC pic14 has no libc (manual 4.9.8). epic-cc ships a
  freestanding one (ADR-018).

## 2. Licensing: ideas yes, code no

SDCC is GPL. It enters the epic in two roles, both consistent with the
existing GPL boundary in AGENTS.md (GPL tools are external-process-only,
never linked or committed):

- **Oracle.** SDCC compiles the same source; we diff observable
  behavior. It lives in the test image, never in the MIT repo.
- **Design reference.** We read SDCC's source for *ideas*: algorithms,
  techniques, data structures, design patterns. Copyright protects
  expression, not ideas, so reimplementing an idea in Rust is legal.

**The line.** Translating SDCC's C into Rust is a derivative work of
GPL code, and the Rust result is GPL. Writing in Rust does not make it
clean. Never copy code text, structure, control flow, variable names,
or comments wholesale.

**The process that keeps us out of the gray zone:**

1. Read SDCC source, write design notes in our own words (the
   algorithm, the tradeoffs, why it works). The notes are ours.
2. Implement from the notes, not from the source. Close the source
   when writing.
3. Never diff our code against SDCC's. No line-by-line porting.
4. Document provenance in the ADR when a technique is genuinely SDCC's:
   "informed by SDCC's approach to X, reimplemented independently."

**One caveat specific to SDCC.** The device libraries under
`device/non-free/` (pic14/pic16 headers and libs) are **not GPL**; they
carry a separate, Microchip-derived license. Copying those is a
different, worse problem. We do not need them: epic-cc generates device
data from Microchip's own DFP (ATDF). The rule is: borrow ideas from
the compiler source (GPL, ideas-only), never from the non-free device
libraries.

**The alternative that changes the calculus.** Relicensing epic-cc
under GPL would allow copying SDCC code outright. Rejected: MIT is more
permissive and the project is MIT. Stated so the decision is explicit,
not accidental.

## 3. Gap inventory

Verified against the SDCC 4.6.2 manual, `pic14devices.txt`,
`supported-devices.ac`, and epic-cc's tree. Items marked **verify** are
ones where the corpus (P0) is the arbiter: clang's frontend may already
close them untested.

### PIC18 (SDCC "pic16" port) — epic-cc gaps

| Capability | SDCC | epic-cc | Note |
|---|---|---|---|
| C89 core, 8/16/32-bit ints, float, pointers, arrays, structs, varargs, switch, function pointers, inline asm, `#pragma config` | ✅ | ✅ | parity already |
| 64-bit `long long` | ✅ | ❌ | backend work (i64 ops) |
| `double` (64-bit float) | ✅ | ❌ | f32 only; **verify** SDCC's default width |
| Bit-fields | ✅ | untested | clang lowers to shift/mask IR we already handle; **verify** |
| Unions | ✅ | partial | `%union.` globals parse (#165); locals **verify** |
| Recursion / reentrancy (LARGE stack model) | ✅ | ❌ by design | `Slot::Frame` hook exists (docs/29 D-2); real gap to close |
| malloc / heap | ✅ | ❌ | library work |
| math.h | ✅ | ❌ | library work |
| `printf` `%f` | ✅ | ❌ | stdio_c.rs: "No floats" |
| Code/eeprom pointers (3-byte generic) | ✅ | ❌ | const via TBLRD only |
| Two-vector priority interrupts | ✅ | ❌ | single-vector compat only (ADR-013 follow-up) |
| `__shadowregs`, `__wparam` | ✅ | ❌ | perf features |
| Real diagnostics vs panics | ✅ | ❌ | known gap |
| EEPROM access | ✅ | ❌ | **verify** |
| Memory models (small/large) | ✅ | ❌ | static overlay only |

### PIC14 (SDCC "pic14" port) — epic-cc gaps

| Capability | SDCC | epic-cc | Note |
|---|---|---|---|
| 16F877A / 16F887 | ✅ | ✅ | parity already |
| C89 core, ints, float, varargs, interrupts, inline asm, SFR headers | ✅ | ✅ | parity already |
| Struct/union as param/return | ❌ | ✅ | **epic-cc ahead** |
| libc | ❌ | ✅ | **epic-cc ahead** |
| Bit-fields | ✅ | untested | **verify** |
| Unions | ✅ | partial | **verify** |
| 64-bit | ❌ | ❌ | no gap |
| Enhanced core (16F193x) | experimental (`libsdcce`) | in progress | the PIC14E sub-epic |

### PIC14E — the whole core

SDCC: experimental support (16F193x, 12F1822, separate `libsdcce`).
epic-cc: the port is **in progress** (docs/33, issue #228). P1 (asm
encoder + sim core) and P2 (integer spine + BSR/MOVLB banking) have
landed; P3-P8 remain. The sub-epic is: finish P3-P8, then reach SDCC
parity on that core.

## 4. The oracle infrastructure (P0)

**SDCC from source, digest-pinned, in the image** — exactly the gputils
pattern:

- SDCC **4.6.0** (latest release, June 2026) source tarball, sha256
  pinned, built in the Dockerfile.
- Build deps to add to `base`: `libboost-dev`, `bison`, `flex` (SDCC's
  build needs them; gputils 1.5.2 is already built — **verify** its
  version is what SDCC 4.6.0's pic ports expect).
- SDCC's regression suite vendored into the image (GPL, never
  committed).
- gpsim as an optional fallback if our sim cannot run SDCC output (P0
  verifies; our sim already has Pic14+Pic18 cores and parses Intel HEX,
  and gplink emits standard HEX, so it should just work).

**The harness** (a new crate or script, mirroring the fuzz
differential):

- For each corpus program: compile with epic-cc to hex; compile with
  `sdcc -mpic16 -p18f4550` (or `-mpic14`) through gplink to hex; load
  both into our sim; seed identical inputs; compare final RAM, ports,
  and cycle count; record flash words + RAM bytes from both.
- Output: per-program table (flash, RAM, cycles, pass/fail) + aggregate
  ratios, published to the CI step summary like the size-regression
  job.

**The corpus** (committed, MIT-clean, ours):

- Tier 1: the existing e2e fixtures (61 on PIC14, 15+ on PIC18).
- Tier 2: one program per SDCC capability — bit-fields, unions, 64-bit,
  double, malloc, math, code/eeprom pointers, priority interrupts,
  recursion, `%f` — each with a hand-computed expected result.
- Tier 3 (image-only): SDCC's own regression suite, run against both
  compilers, pass-rate comparison.

## 5. Definition of done (per family sub-epic)

All five must hold:

1. **Conformance.** Every Tier-2 corpus program compiles under epic-cc
   and sim-verifies to its hand-computed result.
2. **Differential.** For every corpus program SDCC accepts, epic-cc's
   sim-observed final state (RAM, ports, cycle count) matches SDCC's,
   given identical seeded inputs.
3. **Size.** On every corpus program, epic-cc flash words <= SDCC flash
   words and RAM bytes <= SDCC RAM bytes. (Interim: a documented
   per-program ratio, tracked in CI, tightened to <=.)
4. **Speed.** On every corpus program, epic-cc cycle count <= SDCC
   cycle count. (Same interim mechanism.)
5. **Surface.** Every SDCC capability on that target is either
   implemented with a Tier-2 test, or listed in the design as a
   deliberate non-goal with a written rationale.

**Epic-level DoD:** all three family sub-epics green, and the
comparison table is a CI artifact that fails on regression. Nothing
disassembles SDCC output; every item is black-box.

## 6. Epic / sub-epic structure on the board

epic-tasks has no native epic type and should not grow one. The
established pattern (docs/31, epic-hal#59) is tracking issues + labels
+ blockedBy:

- **Umbrella tracking issue** "SDCC parity epic" (epic-cc).
- **Sub-epic tracking issues**: "PIC18 SDCC parity", "PIC14 SDCC
  parity", "PIC14E port + parity", "SDCC oracle infrastructure".
- **Labels**: `epic:sdcc-parity` on everything; `family:pic14` /
  `family:pic18` / `family:pic14e` per item.
- **Edges**: every work item `blockedBy` its sub-epic; every sub-epic
  `blockedBy` the infrastructure issue. `area:*` labels keep agents
  from colliding as today.
- **No epic-tasks code changes.** Phases stay data; grouping is labels.

## 7. Processes

1. **Baseline first.** P0 runs the full comparison before any gap work.
   The honest starting table is the anchor for "when we're done". Some
   inventory rows marked **verify** may already be green (clang's
   frontend does real work); the baseline decides.
2. **One issue per gap**, DoD = its Tier-2 test passing + differential
   clean. Correctness gaps (SDCC can, we can't) before size/cycle gaps
   (we can, but worse).
3. **Review gate unchanged**: develop, separate reviewer, takeoff, PR.
4. **CI**: nightly full comparison (both compilers, all tiers, versions
   recorded in the output); PR runs the committed corpus only. A ratio
   regression fails CI.
5. **Pinning**: SDCC 4.6.0 + gputils versions recorded in every
   comparison output, so the baseline is reproducible.

## 8. Sequencing

- **P0** — SDCC oracle + harness + baseline (blocks everything).
- **P1** — PIC18 parity (the stated priority; biggest surface).
- **P2** — PIC14 parity (small; two axes already won).
- **P3** — PIC14E port + parity (P1-P2 landed; finish P3-P8, then
  parity).

## 9. Where SDCC's ideas help most

The process rule (section 2) slots into each gap ticket's design
phase: read SDCC for the idea, write our own design notes, implement
from the notes. The highest-value targets:

- **Recursion / frame model** (PIC18): SDCC's pic16 port has a real
  FSR1/FSR2 stack model. Our `Slot::Frame` hook (docs/29 D-2) is the
  same idea; reading SDCC informs the design directly.
- **64-bit `long long`**: SDCC's runtime routines are a reference for
  the approach, not the code.
- **malloc / math.h**: SDCC's library design (heap structure, math
  routines) is a legitimate design reference.
- **Bit-fields, unions, `%f`**: mostly clang-frontend work, but SDCC's
  lowering choices are worth reading.
- **Bank-selection minimization**: SDCC has real work here; our
  linear-tracking gap (README) is exactly where their technique is
  informative.
