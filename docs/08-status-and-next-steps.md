# 08 — Status and next steps

**Start here if you are resuming cold.**

Last updated: 2026-08-14

---

## Where we are

**No compiler code is implemented.** The repository contains documentation plus a working
build environment. There is no `Cargo.toml` and no crates yet.

The Nix dev shell **is** built and verified — see
[`09-build-environment.md`](09-build-environment.md). `direnv allow`, then you have pinned
clang 20.1.8, rustc 1.97.1, gpasm 1.5.2, cvise, creduce, and csmith.

We are at the end of a design conversation, following a brainstorm → design → approve →
implement flow. The state of that flow:

| Step | Status |
|---|---|
| Explore project context | ✅ done |
| Clarifying questions (4 asked, 4 answered) | ✅ done |
| Propose 2–3 approaches with trade-offs | ✅ done — Approach A chosen |
| Online prior-art survey | ✅ done — [`02-prior-art.md`](02-prior-art.md) |
| Reference books obtained | ✅ done — Muchnick + lcc |
| Documentation phase | ✅ done — this `docs/` tree |
| Build environment (Nix flake) | ✅ done and verified — [ADR-007](03-decisions.md), [`09`](09-build-environment.md) |
| Present design in sections, approve each | ⚠️ **Rust approved (ADR-005). The ten-stage pipeline is presented but NOT approved. Sections 2–4 never presented.** |
| Write design doc / spec | ⏸ superseded in part by this `docs/` tree |
| Implementation plan | ❌ not started |
| **Feasibility spike** | 🟡 **approved and STARTED, paused partway — 2 of 4 questions answered. See [`10-spike-findings.md`](10-spike-findings.md)** |

## What the user has explicitly decided

These are settled. Do not re-litigate them without new evidence.

1. **Goal:** a usable compiler for real PIC16F877A projects — write C, flash it, hardware
   works. Not a research artifact, not an XC8 clone.
2. **Toolchain:** whole-program compilation, we own everything down to Intel HEX.
3. **C surface:** all of it — core C89 + 8/16-bit ints, 32-bit `long` with soft arithmetic,
   soft-float, and interrupts with SFR headers.
4. **Architecture:** Approach A — clang as an out-of-process front end emitting `.ll` text;
   custom whole-program PIC14 backend. Not an LLVM backend.
5. **De-risking:** spike the backend spine **before** writing the full plan.
6. **Commits:** conventional commits, single line, at most 3 lines.
7. **Implementation language:** Rust ([ADR-005](03-decisions.md)).
8. **Build isolation:** Nix flake + direnv, nothing installed system-wide; clang pinned to
   20.1.8 ([ADR-007](03-decisions.md)).
9. **Vendored material:** user supplies Microchip installers, datasheets, and the reference
   books under `vendor/`, gitignored ([`../vendor/README.md`](../vendor/README.md)).

## What is presented but NOT yet approved

- The **ten-stage pipeline** ([`04-pipeline-design.md`](04-pipeline-design.md)). Presented
  to the user; they redirected to research, documentation, and build setup before
  answering. **Ask before building to it.**
- The **device-description-as-data** approach ([ADR-004](03-decisions.md)).

## What was never presented at all

Design sections 2–4 were outlined internally but never shown to the user:

- **Section 2** — the allocator / banking core in detail
- **Section 3** — the verification harness (partially captured in [`05-verification.md`](05-verification.md))
- **Section 4** — phasing and milestones

---

## ⏭️ The immediate next action: finish the feasibility spike

> **Approved and in progress. Paused 2026-08-14 partway through.**
> Q2 (IR surface) and Q3 (storage pressure) have real answers; Q1 and Q4 do not.
> **Read [`10-spike-findings.md`](10-spike-findings.md) — it has a step-by-step
> "How to resume" section.** The remaining work is storage allocation, phi elimination,
> instruction selection, and a simulator.

The original scope, for reference:

It is a **spike**: its output is an *answer*, not code we keep. Everything built is
throwaway and must be labelled as such.

### The probe

Install clang and gputils. Compile a small program containing **a loop, an `if`, a function
call, and 16-bit arithmetic** to `.ll` at a 16-bit-`int` datalayout. Hand-write a throwaway
Rust pipeline that:

1. parses that `.ll`
2. statically allocates locals into the common RAM region 0x70–0x7F
3. selects a minimal instruction set
4. emits `.asm` + `.hex`

Verify with a throwaway PIC14 simulator, and cross-check the HEX against `gpasm`'s
assembly of our `.asm`.

### The four questions it must answer

1. **Is `.ll` text actually a good substrate?** Or does clang bake in ABI decisions
   (`byval`, `sret`, varargs, alloca patterns) that fight a machine with no stack?
2. **How much `.ll` surface must we parse** for a realistic program — is a hand-written
   parser tractable, or a tarpit that grows without bound?
3. **Does common-RAM-as-imaginary-registers survive contact with real IR?** We have 16
   bytes; llvm-mos gives the 6502 thirty-two.
4. **How bad is Harvard in practice?** `const` tables in program memory are the one problem
   the 6502 never had, so llvm-mos offers no prior art.

### Success criterion

**One program containing all four constructs compiles and runs correctly in simulation.**

Failure is equally informative and equally cheap. If the answer is "`.ll` is unworkable,"
the fallback is Approach C (our own C front end) — **not** the LLVM backend route, which
[`02-prior-art.md`](02-prior-art.md) rules out on evidence.

---

## Proposed phasing after the spike (never presented — needs user approval)

Sequenced so the hard, high-risk parts come early and the large-but-decoupled parts come
last.

1. **Verification harness first** — our PIC14 simulator, the `xc8-cc` differential runner,
   `gpasm` cross-check, snapshot infrastructure. Build the oracle before the thing it
   judges.
2. **Integer spine** — core C89, 8/16-bit ints, control flow, non-recursive calls. Overlay
   allocation and BANKSEL/PAGESEL minimisation land here. This is the bulk of the
   difficulty.
3. **Pointers, arrays, structs** — the `FSR`/`INDF` codegen problem.
4. **Interrupts + SFR headers + device description** — makes it actually usable on hardware.
5. **32-bit `long`** + soft mul/div/mod runtime.
6. **Random testing at scale** — YARPGen + cvise loop running unsupervised.
7. **Soft-float** — largest library chunk, least coupled to the hard backend problems.

## Open questions

- **Datalayout:** use MSP430's wholesale (`-target msp430`), or declare a custom one? We
  only consume IR, so we are free either way. Settle during the spike.
- **Which clang optimization passes to enable.** `-O2`/`-Oz` wholesale is wrong. Three
  known costs: SROA increases RAM pressure on a 368-byte machine; the optimizer normalises
  shifts-and-adds into multiplies we must re-expand; and — measured during environment
  setup, see [`09`](09-build-environment.md) — `-Oz` emits **arbitrary-precision integer
  types** (an actual `mul i17`) and **intrinsics** (`@llvm.smax.i16`). Our legalizer
  therefore cannot assume 8/16/32-bit widths. Needs a curated pass list, measured by the
  spike.
- **Legalizer generality.** Directly following from the above: how general does the
  widening/narrowing story need to be? A `mul i17` on a core with no hardware multiply is
  an unpleasant lowering, and it appeared in a two-function test program.
- **Every `[VERIFY]` item in [`01-target-pic14.md`](01-target-pic14.md)** — memory map, bank
  ranges, common RAM extent, flash size, `const`-in-flash access mechanism. Confirm against
  DS39582 and DS33023 before hard-coding into the device file.
- **Interrupt/main shared-function policy:** duplicate such functions, or give them
  non-overlapping frames? Affects both RAM pressure and code size.
- **Recursion:** confirmed a compile error. Should there be an escape hatch
  (e.g. an explicit software-stack attribute) for the rare case? Probably not for v1.

## Corrections made during the design conversation

Recorded so they are not silently re-introduced:

- **"LLVM cannot target accumulator machines" is false.** llvm-mos disproves it. The real
  argument against the LLVM route is *cost*, not *possibility*. See [ADR-001](03-decisions.md).
- **"gputils is largely unmaintained" is false.** v1.5.2 shipped 2025-10-23 and it is
  actively maintained. It is a useful oracle and device-data source.
- **"XC8 is clang-based, so its PIC16 codegen is LLVM" is false.** clang is the front end;
  `cgpic` (HI-TECH lineage) is the mid-range code generator.
