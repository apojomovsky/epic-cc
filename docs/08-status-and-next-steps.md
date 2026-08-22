# 08 - Status and next steps

**Start here if you are resuming cold.** This is the current-state map; the detailed
designs and their ADRs are the source of truth.

Last updated: 2026-08-22

---

## Where we are

The design conversation from 2026-08-14 is closed and implemented. The repository
now contains a complete compiler, not just documentation:

- **Build environment:** docker multi-stage toolchain, verified. `make image` builds the
  `dev` image with pinned clang 20.1.8, rustc 1.97.1, gpasm 1.5.2, csmith, creduce and
  cvise. See [`09-build-environment.md`](09-build-environment.md) and
  [ADR-008](03-decisions.md).
- **PIC14 backend:** feature-complete for the v1 C surface. Core C89, 8/16/32-bit ints,
  pointers, arrays, structs, `const` in flash via `RETLW` tables, frame overlay across
  the whole-program call graph, multi-bank RAM and multi-page flash, interrupts with
  SFR headers, and IEEE-754 single soft-float. Every feature has an e2e fixture under
  `crates/driver/tests/fixtures` and a gpasm byte-for-byte cross-check.
- **PIC18 backend:** P0 through P8 landed per [`29-pic18-port-design.md`](29-pic18-port-design.md).
  `device` abstraction, PIC18 `asm` encoder and `sim` core, integer spine with Access
  Bank and `BSR` banking, pointers via `FSR0`/`INDF0` ([ADR-009](adr/ADR-009-pic18-pointer-model.md)),
  `const` via `TBLRD` ([ADR-010](adr/ADR-010-pic18-const-tblrd.md)),
  interrupts in single-vector compat mode ([ADR-013](adr/ADR-013-pic18-interrupts.md)),
  32-bit `long` with hardware `MULWF` ([ADR-014](adr/ADR-014-pic18-hw-arithmetic-routines.md)),
  soft-float ([ADR-015](adr/ADR-015-pic18-softfloat.md)),
  and a device-threaded differential fuzz gate ([ADR-016](adr/ADR-016-pic18-fuzz-gate.md)).
- **Multi-TU and distribution:** `llvm-link` merge and `epic-cc` binary naming per
  [ADR-011](adr/ADR-011-multi-tu-front-end.md); silicon-real codegen (`EPIC_AT`,
  `EPIC_CONFIG`, `EPIC_FOSC_HZ`) per [ADR-012](adr/ADR-012-cc3-silicon-real-codegen.md);
  freestanding libc headers per [ADR-018](adr/ADR-018-cc2-freestanding-libc.md);
  inline assembly (rungs 1-4) per [ADR-017](adr/ADR-017-cc4-inline-assembly.md).
  Public binary distribution is designed in [`30-distribution-design.md`](30-distribution-design.md);
  ecosystem integration (epic-cc as the default toolchain for epic-hal, PlatformIO)
  is designed in [`31-ecosystem-integration-design.md`](31-ecosystem-integration-design.md).

What used to live in this document, the phasing table, the "never presented" sections,
and the open-questions list, is superseded by [`12-backend-design.md`](12-backend-design.md)
(the approved PIC14 spec), [`29-pic18-port-design.md`](29-pic18-port-design.md),
[`30-distribution-design.md`](30-distribution-design.md),
[`31-ecosystem-integration-design.md`](31-ecosystem-integration-design.md) and the
ADRs in [`03-decisions.md`](03-decisions.md) plus `docs/adr/`.

## What the user has decided (still settled)

1. **Goal:** a usable compiler for real PIC16F877A projects, write C, flash it,
   hardware works. Not a research artifact, not an XC8 clone.
2. **Toolchain:** whole-program compilation, we own everything down to Intel HEX.
3. **C surface:** all of it, core C89 + 8/16-bit ints, 32-bit `long` with soft
   arithmetic, soft-float, and interrupts with SFR headers.
4. **Architecture:** Approach A, clang as an out-of-process front end emitting `.ll` text;
   custom whole-program PIC14 backend. Not an LLVM backend.
5. **Commits:** conventional commits, single line, at most 3 lines.
6. **Implementation language:** Rust ([ADR-005](03-decisions.md)).
7. **Build isolation:** docker multi-stage toolchain, nothing installed system-wide;
   clang pinned to 20.1.8 ([ADR-008](03-decisions.md)).
8. **Vendored material:** user supplies Microchip installers, datasheets, and the reference
   books under `vendor/`, gitignored ([`../vendor/README.md`](../vendor/README.md)).

## Where to go next

| If you want | Read |
|---|---|
| The PIC14 architecture and why it is hard | [`01-target-pic14.md`](01-target-pic14.md) |
| The consolidated backend spec | [`12-backend-design.md`](12-backend-design.md) |
| The ten-stage pipeline and stage contracts | [`04-pipeline-design.md`](04-pipeline-design.md) |
| Verification: simulator, gpasm oracle, fuzzing | [`05-verification.md`](05-verification.md) |
| Build environment and pinned versions | [`09-build-environment.md`](09-build-environment.md) |
| PIC18 port: why it is smaller than a second compiler, and its phases | [`29-pic18-port-design.md`](29-pic18-port-design.md) |
| Public binary distribution | [`30-distribution-design.md`](30-distribution-design.md) |
| Ecosystem integration: epic-cc + epic-hal + PlatformIO | [`31-ecosystem-integration-design.md`](31-ecosystem-integration-design.md) |
| Architecture decisions and rejected alternatives | [`03-decisions.md`](03-decisions.md) and `docs/adr/` |

Implementation plans (`docs/superpowers/plans/`) are ephemeral and never tracked on
`master` (see `AGENTS.md`); load-bearing decisions are distilled into ADRs. The
numbered milestone plans `docs/13-` through `docs/28-` are the historical record of
the PIC14 spine and remain as reference.

## Corrections that still apply

- **"LLVM cannot target accumulator machines" is false.** llvm-mos disproves it. The
  argument against the LLVM route is cost, not possibility. See [ADR-001](03-decisions.md).
- **"gputils is largely unmaintained" is false.** v1.5.2 shipped 2025-10-23 and it is
  actively maintained. It is a useful oracle and device-data source.
- **"XC8 is clang-based, so its PIC16 codegen is LLVM" is false.** clang is the front end;
  `cgpic` (HI-TECH lineage) is the mid-range code generator.
