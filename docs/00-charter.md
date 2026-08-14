# 00 — Project charter

Date established: 2026-08-14

## Goal

Build a C compiler that produces **working, flashable code for real PIC16F877A projects**.

The user's stated success condition, chosen explicitly over three alternatives:

> "Usable compiler for real 877A projects — I want to write C, flash it, and have my
> hardware work. Code quality must be competitive enough to fit in 8K words. Beating XC8
> free-mode optimization is a nice-to-have, not the point."

### What this goal rules out

Three alternative framings were offered and **rejected**:

- *Research/learning artifact* — elegance over shipping. Not the goal.
- *XC8 output-compatible clone* — bit-similar output to XC8, validated by differential
  testing. Rejected as the goal, though differential testing against XC8 is retained as a
  *verification technique* (see [`05-verification.md`](05-verification.md)).
- *Agent-autonomy testbed* — the compiler as a vehicle for testing unsupervised agent work.
  Not the primary goal, but unsupervised agent operation **is** a stated end objective, so
  the design must support it. See "Autonomy requirement" below.

## Scope decisions (all explicitly chosen by the user)

### Toolchain boundary — we own everything

Compile all `.c` files at once, perform call-graph overlay allocation with full program
visibility, emit `.asm` for human inspection **and** assemble/link to Intel HEX ourselves.
No external assembler or linker dependency in the shipping product.

Rejected: emitting `.asm` for `gputils` (gpasm/gplink), and emitting `.asm` for Microchip's
`pic-as`. Rationale in [`03-decisions.md`](03-decisions.md) (ADR-002).

Note that `gpasm` is still used as a *test-time cross-check oracle*. That is a different
thing from depending on it.

### C language surface — all of it

The user selected every option offered:

| Feature | Status |
|---|---|
| Core C89, 8/16-bit ints, pointers, structs, unions, arrays, enums, all control flow, non-recursive calls | Required |
| 32-bit `long` with soft mul/div/mod | Required |
| `float` (IEEE-754 single, soft-float) | Required |
| Interrupts: `__interrupt` functions, context save/restore, `volatile` correctness, 877A SFR headers, register bit-fields, separate ISR overlay region | Required |

**Phasing note:** this is the full target, not the v1 target. Float is the largest library
chunk and the least coupled to the hard backend problems, so it is sequenced last. See
[`08-status-and-next-steps.md`](08-status-and-next-steps.md).

### Compiler architecture — clang front end, custom backend

Approach A of three considered. clang runs out-of-process and emits LLVM IR *as text*;
we parse the `.ll` and implement our own whole-program PIC14 backend. We never link
libLLVM and never touch SelectionDAG, GlobalISel, TableGen, or MCTargetDesc.

Rejected: forking SDCC's pic14 port; writing our own C front end from day one. Rationale
and the evidence behind it in [`03-decisions.md`](03-decisions.md) (ADR-001).

## Non-goals for v1

- **Separate compilation.** Whole-program analysis is the entire point; a traditional
  compile-then-link model fights overlay allocation.
- **Debugger / COFF / ELF output.** HEX, listing, and map files only.
- **Device breadth.** PIC16F877A first. Other PIC14 parts are a data change, not a code
  change, by design — but they are not v1.
- **C99/C11 breadth beyond what the front end gives us free.** We inherit whatever clang
  parses; we do not chase conformance corners the target cannot express.

## Autonomy requirement

A stated end objective is that **an agent can work on this unsupervised**. This is a real
design constraint, not a nice-to-have, and it drives several decisions:

- Every pipeline stage boundary is a **diffable text artifact**, so a miscompile can be
  bisected to a stage before anyone reads code.
- The verification harness (simulator + dual oracles + random program generation +
  automatic test-case reduction) is prioritised *early*, so the agent can manufacture its
  own minimal failing test cases with no human in the loop.
- We avoid a permanent fork of a 30-million-line C++ tree, because a perpetual upstream
  rebase is close to a worst-case environment for unsupervised work. This is a major part
  of why the LLVM-backend route was rejected.

## Legal and ethical constraints

- **No reverse engineering of XC8 binaries.** The user initially proposed disassembling
  XC8 to reproduce its behaviour. This was flagged and redirected: the XC8 licence forbids
  RE, and black-box differential testing (compile the same source, diff the observable
  behaviour) is both legally clean and strictly more useful, since it yields a permanent
  regression oracle. Everything genuinely load-bearing is public anyway — datasheets, the
  ISA, the XC8 user guide's ABI chapter, and SDCC/gputils source.
- **GPL boundary.** `gputils` and `gpsim` are GPL. Using them as external processes in a
  test harness does not affect our licensing. Linking them would.
- **Copyrighted references stay out of the repo.** See [`06-environment.md`](06-environment.md).
