# pic8_compiler

A C compiler for mid-range 8-bit Microchip PIC microcontrollers (PIC14 core), targeting
the **PIC16F877A** first.

> **Status as of 2026-08-14: design phase. No code has been written yet.**
> The architecture is decided and the prior-art survey is complete. The immediate next
> action is a throwaway feasibility spike. See
> [`docs/08-status-and-next-steps.md`](docs/08-status-and-next-steps.md).

## What this is

A whole-program C compiler that takes `.c` files and emits Intel HEX, owning every stage
from LLVM IR down to the assembler. It uses **clang as an out-of-process front end** and
implements a **custom PIC14 backend** — deliberately *not* an LLVM backend. The reasoning,
including the prior project that died attempting the LLVM route, is in
[`docs/02-prior-art.md`](docs/02-prior-art.md) and [`docs/03-decisions.md`](docs/03-decisions.md).

## Read these in order

| Doc | What it covers |
|---|---|
| [`docs/00-charter.md`](docs/00-charter.md) | Goal, scope, non-goals, and the decisions the user has explicitly made |
| [`docs/01-target-pic14.md`](docs/01-target-pic14.md) | The PIC14 architecture and exactly why it is hostile to C |
| [`docs/02-prior-art.md`](docs/02-prior-art.md) | Survey: llvm-mos, llvm-pic, SDCC, XC8, gputils, gpsim, key papers |
| [`docs/03-decisions.md`](docs/03-decisions.md) | Architecture decision records, with rejected alternatives and rationale |
| [`docs/04-pipeline-design.md`](docs/04-pipeline-design.md) | The ten-stage compiler pipeline (partially approved — see status) |
| [`docs/05-verification.md`](docs/05-verification.md) | Oracles, simulator, differential testing, fuzzing, auto-reduction |
| [`docs/06-environment.md`](docs/06-environment.md) | Toolchain setup, the XC8 install, **how to read the reference PDFs** |
| [`docs/07-references.md`](docs/07-references.md) | Books, papers, datasheets, URLs |
| [`docs/08-status-and-next-steps.md`](docs/08-status-and-next-steps.md) | **Start here if you are resuming cold.** Where we are, what is next, open questions |

## Quick orientation for a resuming agent

1. Read [`docs/08-status-and-next-steps.md`](docs/08-status-and-next-steps.md) first.
2. Nothing is implemented. The repo contains documentation only.
3. The user's approval gates are real: the next step (a spike) has been scoped and
   presented but **not yet started**. Do not begin implementation without confirming.
