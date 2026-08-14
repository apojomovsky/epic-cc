# 05 — Verification strategy

This is the part that makes unsupervised agent work viable, and it is deliberately
prioritised **early** — before most of the compiler.

The goal is a closed loop that **manufactures its own minimal failing test cases with no
human in the sequence**:

```
random program generator ──► our compiler ──┐
                        └──► oracle compiler ┴──► simulator ──► differ ──► cvise ──► minimal repro
```

## Two independent oracles

### 1. XC8 (`xc8-cc`)

Installed at `/opt/microchip/xc8/v4.00/` — see [`06-environment.md`](06-environment.md).
Compile the same source with XC8, run both outputs on our simulator, compare observable
state (RAM contents, port writes, cycle counts).

**Black-box only.** Never disassemble or reverse-engineer the XC8 binaries — see
[ADR-006](03-decisions.md). We observe its *output*, not its internals.

XC8 free mode applies no optimization limits that affect *correctness*, which makes it a
sound correctness oracle even in its unlicensed configuration.

### 2. gpsim

Version 0.32.1 (Nov 2023), supports the 14-bit core. GPL — invoke as an external process
only, never link. Useful as an independent check on our own simulator's semantics: if our
simulator and gpsim disagree about what a program does, one of them is wrong, and that is
worth knowing before we blame the compiler.

### 3. gpasm (assembler cross-check)

gputils v1.5.2 (2025-10-23), actively maintained. Assemble our emitted `.asm` with `gpasm`
and diff the resulting HEX against our own assembler's output. This isolates
"our assembler is wrong" from "our codegen is wrong" — two failure modes that otherwise
look identical.

## Our own simulator

Despite gpsim existing, we plan our own PIC14 instruction-set simulator.

**Why:** 35 instructions makes it small (~1500 lines). Owning it gives us determinism,
speed, embeddability in `cargo test`, cycle counting, and the ability to assert on internal
state directly. It also removes a GPL process boundary from the inner test loop.

**Why gpsim still matters:** as an independent semantic reference to validate our simulator
against. Our simulator being wrong in the same way our compiler is wrong is the one failure
mode that a single-oracle setup cannot catch.

## Random program generation

**YARPGen** is preferred over Csmith.

- Generates **UB-free** programs by construction, using generation policies to increase
  diversity and trigger more optimizations. Found 220+ bugs in GCC, LLVM, and ICC.
- Csmith leans on 32-bit-`int` assumptions and UB-adjacent patterns that would waste
  substantial time on a 16-bit-`int` target. Csmith-generated programs also contain a lot
  of dead code that gets eliminated, yielding little actual machine code.

Generation must be **constrained to our supported C subset** and to the 877A's resource
limits (368 bytes RAM, 8K words flash) — an unconstrained generator will mostly produce
programs that legitimately do not fit.

## Automatic test-case reduction

**C-Reduce / cvise.** Per Regehr et al. (PLDI'12), outputs average **>25× smaller** than
delta debugging or other reducers, and avoid the classic failure of reducing a program into
undefined behaviour.

Interface: an **oracle shell script** that returns whether the unwanted behaviour still
occurs. Writing good oracle scripts is the skill here — they must distinguish "still
miscompiles" from "now fails for an unrelated reason."

This is the component that most directly enables unsupervised operation: it converts "a
10,000-line random program produces the wrong answer" into "here are 15 lines that
miscompile," automatically.

## Stage-boundary snapshot testing

Because every pipeline stage boundary is a text artifact ([`04-pipeline-design.md`](04-pipeline-design.md)),
snapshot tests (`insta`) can pin each stage's output. A miscompile is then bisected to a
stage *before* anyone reads code.

## Hardware-in-the-loop

The final oracle is real silicon. HEX files can be flashed via MPLAB IPE. This is out of
scope for automated loops but is the ultimate acceptance test for
[`00-charter.md`](00-charter.md)'s goal ("write C, flash it, have my hardware work").

## Licensing boundary — important

`gputils` and `gpsim` are **GPL**. Invoking them as external processes from a test harness
does not affect our licensing. **Linking them into our compiler would.** Keep them behind a
process boundary.
