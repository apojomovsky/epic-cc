# Internal design history

Design notes, decision records and specs that shaped epic-cc. Start with
[`design/08-status-and-next-steps.md`](design/08-status-and-next-steps.md) for the current state.
User-facing documentation stays under `docs/` (see [`../../README.md`](../../README.md)).
Old paths are listed in [`MIGRATION.md`](MIGRATION.md).

## Architecture decision records

- [`ADR-009-pic18-pointer-model.md`](adr/ADR-009-pic18-pointer-model.md): ADR-009: PIC18 pointer model (shared GEP fold, single FSR0, no PLUSWn)
- [`ADR-010-pic18-const-tblrd.md`](adr/ADR-010-pic18-const-tblrd.md): ADR-010: PIC18 const via TBLRD (DB-packed flash, per-byte TBLPTR re-setup)
- [`ADR-011-multi-tu-front-end.md`](adr/ADR-011-multi-tu-front-end.md): ADR-011: Multi-translation-unit front end (llvm-link merge, sanitize in irparse)
- [`ADR-012-cc3-silicon-real-codegen.md`](adr/ADR-012-cc3-silicon-real-codegen.md): ADR-012: CC-3 silicon-real codegen: clock derivation, HEX emission, and fixes
- [`ADR-013-pic18-interrupts.md`](adr/ADR-013-pic18-interrupts.md): ADR-013: PIC18 interrupts (single-vector compatibility mode, MOVFF save area)
- [`ADR-014-pic18-hw-arithmetic-routines.md`](adr/ADR-014-pic18-hw-arithmetic-routines.md): ADR-014: PIC18 arithmetic routines (hardware MULWF, branch-based divmod)
- [`ADR-015-pic18-softfloat.md`](adr/ADR-015-pic18-softfloat.md): ADR-015: PIC18 soft-float (port of PIC14 recipes, skip-sensitive frame rule)
- [`ADR-016-pic18-fuzz-gate.md`](adr/ADR-016-pic18-fuzz-gate.md): ADR-016: PIC18 fuzz gate (device-threaded differential runner)
- [`ADR-017-cc4-inline-assembly.md`](adr/ADR-017-cc4-inline-assembly.md): ADR-017: CC-4 inline assembly (naked, module asm, opaque statement blocks)
- [`ADR-018-cc2-freestanding-libc.md`](adr/ADR-018-cc2-freestanding-libc.md): ADR-018: CC-2 freestanding libc subset (stdint, stdbool, stddef, string; stdlib joined 2026-08)
- [`ADR-019-pic-variants-device-registry.md`](adr/ADR-019-pic-variants-device-registry.md): ADR-019 -- PIC variant support: file-per-device TOML + build.rs codegen + --target
- [`ADR-020-dfp-toml-generator.md`](adr/ADR-020-dfp-toml-generator.md): ADR-020 -- DFP -> TOML generator (ATDF/EDC ingestion)
- [`ADR-021-device-provenance-and-cross-check.md`](adr/ADR-021-device-provenance-and-cross-check.md): ADR-021 -- Device data provenance and the gputils cross-check
- [`ADR-022-indirect-calls.md`](adr/ADR-022-indirect-calls.md): ADR-022 -- Indirect calls through function pointers
- [`ADR-024-cross-context-callbacks.md`](adr/ADR-024-cross-context-callbacks.md): ADR-024 -- Cross-context stored callbacks (store in main, invoke in the ISR)
- [`ADR-025-size-map-reporting.md`](adr/ADR-025-size-map-reporting.md): ADR-025 -- Size and map reporting (CC-6, reporting half)
- [`ADR-026-pic18-struct-byte-alignment.md`](adr/ADR-026-pic18-struct-byte-alignment.md): ADR-026 -- PIC18 struct ABI: XC8 byte alignment
- [`ADR-027-schedule-pass.md`](adr/ADR-027-schedule-pass.md): ADR-027 -- Bank-aware instruction scheduler: a new schedule pass
- [`ADR-028-address-to-line-table.md`](adr/ADR-028-address-to-line-table.md): ADR-028 -- Address-to-line table (thread SrcLoc to asm)
- [`ADR-029-pic8-part-catalog.md`](adr/ADR-029-pic8-part-catalog.md): ADR-029 -- PIC8 flash-generation part catalog: one tracking table, popularity tiers, on-demand ports
- [`ADR-030-pic18-priority-interrupts.md`](adr/ADR-030-pic18-priority-interrupts.md): ADR-030 -- PIC18 two-vector priority interrupts
- [`ADR-031-float-frames-like-integer.md`](adr/ADR-031-float-frames-like-integer.md): ADR-031 -- Float frames place like integer ones
- [`ADR-032-baseline-const-tables.md`](adr/ADR-032-baseline-const-tables.md): ADR-032 -- Baseline const tables live in page low halves, readers cost a stack level
- [`ADR-033-pic18-malloc-heap.md`](adr/ADR-033-pic18-malloc-heap.md): ADR-033: freestanding malloc/free over a caller-registered arena (epic-cc#343)
- [`ADR-034-single-region-common-ram-split.md`](adr/ADR-034-single-region-common-ram-split.md): ADR-034 -- Devices whose entire GPR collapses to one physical region get a carved-out common_ram corner
- [`ADR-035-pic18-switch-tables.md`](adr/ADR-035-pic18-switch-tables.md): ADR-035 -- Switch is a first-class IR terminator; PIC18 lowers dense switches to PCL jump tables
- [`ADR-036-pic18-frames-below-globals.md`](adr/ADR-036-pic18-frames-below-globals.md): ADR-036 -- PIC18 places the local frame overlay below the globals
- [`ADR-037-pic18-routine-bank-granularity.md`](adr/ADR-037-pic18-routine-bank-granularity.md): ADR-037 -- PIC18 runtime-routine frames round on the BSR bank, not a region
- [`ADR-038-dispatch-storage-frame-policy.md`](adr/ADR-038-dispatch-storage-frame-policy.md): ADR-038 -- Dispatch-storage frame policy: per-priority copies, no foreign frames
- [`ADR-039-pic18-start-clears-zero-ram.md`](adr/ADR-039-pic18-start-clears-zero-ram.md): ADR-039 -- Zero-initialized RAM is cleared by __start on PIC18
- [`ADR-040-pic14-keeps-globals-first.md`](adr/ADR-040-pic14-keeps-globals-first.md): ADR-040 -- PIC14 keeps globals-first: the dispatch bank-switch sink is not an alloc-placement problem
- [`ADR-041-pic18-exit-bank-carry.md`](adr/ADR-041-pic18-exit-bank-carry.md): ADR-041 -- PIC18 carries provable callee exit banks across calls
- [`ADR-042-pic18-code-factoring.md`](adr/ADR-042-pic18-code-factoring.md): ADR-042 -- PIC18 factors repeated code on the final listing
- [`ADR-043-generated-headers.md`](adr/ADR-043-generated-headers.md): ADR-043 -- Generated device headers and a real xc.h
- [`ADR-044-pic18-subwfb-borrow-chain.md`](adr/ADR-044-pic18-subwfb-borrow-chain.md): ADR-044 -- PIC18 borrow chains use SUBWFB, never SUBFWB
- [`ADR-045-optimization-profiles.md`](adr/ADR-045-optimization-profiles.md): ADR-045 -- Optimization profiles -O0/-O1/-O2/-Os
- [`ADR-046-flash-vtable-tblrd.md`](adr/ADR-046-flash-vtable-tblrd.md): ADR-046 -- Flash-resident vtables via runtime TBLRD (PIC18)
- [`ADR-047-outline-matching-stays-exact-text.md`](adr/ADR-047-outline-matching-stays-exact-text.md): ADR-047 -- Outline matching stays exact-text under frame renumbering
- [`ADR-048-pic18-narrowed-isr-save.md`](adr/ADR-048-pic18-narrowed-isr-save.md): ADR-048 -- PIC18 ISR context save narrows to the reachable clobber set
- [`ADR-049-opcode-level-sim-sweep.md`](adr/ADR-049-opcode-level-sim-sweep.md): ADR-049 -- Opcode-level MPLAB SIM sweep in six shards
- [`ADR-050-pic18-pointer-phi-cycle-refusal.md`](adr/ADR-050-pic18-pointer-phi-cycle-refusal.md): ADR-050 -- PIC18 refuses pointer phi cycles that need a copy temp
- [`ADR-051-short-table-readers.md`](adr/ADR-051-short-table-readers.md): ADR-051 -- Short const-table readers share their base's page
- [`ADR-052-const-search-fold.md`](adr/ADR-052-const-search-fold.md): ADR-052 -- All-constant search loops fold by guarded unrolling
- [`ADR-053-bare-bit-entities.md`](adr/ADR-053-bare-bit-entities.md): ADR-053 -- Bare bit names as section-placed _Bool entities

## Numbered design notes and findings

- [`00-charter.md`](design/00-charter.md): 00: Project charter
- [`01-target-pic14.md`](design/01-target-pic14.md): 01: The target: PIC14 mid-range core / PIC16F877A
- [`02-prior-art.md`](design/02-prior-art.md): 02: Prior art survey
- [`03-decisions.md`](design/03-decisions.md): 03 -- Architecture decision records
- [`04-pipeline-design.md`](design/04-pipeline-design.md): 04: Compiler pipeline design
- [`05-verification.md`](design/05-verification.md): 05: Verification strategy
- [`06-environment.md`](design/06-environment.md): 06: Environment, tooling, and how to read the references
- [`07-references.md`](design/07-references.md): 07: References
- [`08-status-and-next-steps.md`](design/08-status-and-next-steps.md): 08 - Status and next steps
- [`09-build-environment.md`](design/09-build-environment.md): 09: Build environment
- [`10-spike-findings.md`](design/10-spike-findings.md): 10: Feasibility spike: FINDINGS
- [`11-pointer-const-findings.md`](design/11-pointer-const-findings.md): 11: Pointer / const-in-flash spike: FINDINGS
- [`12-backend-design.md`](design/12-backend-design.md): 12: Backend design (approved)
- [`13-phase1-verification-harness-plan.md`](design/13-phase1-verification-harness-plan.md): Phase 1: Verification Harness Implementation Plan
- [`14-integer-spine-m1-plan.md`](design/14-integer-spine-m1-plan.md): Integer Spine: Milestone 1: Pipeline Skeleton Implementation Plan
- [`15-integer-spine-m2-plan.md`](design/15-integer-spine-m2-plan.md): Integer Spine: Milestone 2: Control Flow, Calls, and 16-bit Implementation Plan
- [`16-integer-spine-m3-plan.md`](design/16-integer-spine-m3-plan.md): Integer Spine: Milestone 3: Overlay Allocation Implementation Plan
- [`17-integer-spine-m4-plan.md`](design/17-integer-spine-m4-plan.md): Integer Spine: Milestone 4: Real Banking Implementation Plan
- [`18-phase3-pointers-const-plan.md`](design/18-phase3-pointers-const-plan.md): Phase 3: Milestone 5: Pointers, Arrays, and Const-in-Flash Implementation Plan
- [`19-integer-spine-m6-scalar-plan.md`](design/19-integer-spine-m6-scalar-plan.md): Integer Spine: Milestone 6: Scalar C Surface Completion Implementation Plan
- [`20-integer-spine-m7-structs-plan.md`](design/20-integer-spine-m7-structs-plan.md): Integer Spine: Milestone 7: Structs (phase-3 completion) Implementation Plan
- [`21-integer-spine-m8-runtime-muldiv-plan.md`](design/21-integer-spine-m8-runtime-muldiv-plan.md): Integer Spine: Milestone 8: mul/div/shift Runtime Surface Implementation Plan
- [`22-integer-spine-m9-multibank-fsr-plan.md`](design/22-integer-spine-m9-multibank-fsr-plan.md): Integer Spine: Milestone 9: Multi-Bank FSR+IRP Arrays Implementation Plan
- [`23-integer-spine-m10-pclath-tables-plan.md`](design/23-integer-spine-m10-pclath-tables-plan.md): Integer Spine: Milestone 10: Const-Table PCLATH Page-Crossing Implementation Plan
- [`24-integer-spine-m11-multipage-plan.md`](design/24-integer-spine-m11-multipage-plan.md): Integer Spine: Milestone 11: Multi-Page Code Model Implementation Plan
- [`25-integer-spine-m12-long-plan.md`](design/25-integer-spine-m12-long-plan.md): Integer Spine: Milestone 12: 32-bit long Implementation Plan
- [`26-phase4-interrupts-plan.md`](design/26-phase4-interrupts-plan.md): Integer Spine: Milestone 13: Interrupts + SFR/Device Support Implementation Plan
- [`27-phase6-random-testing-plan.md`](design/27-phase6-random-testing-plan.md): Integer Spine: Milestone 14: Random Testing at Scale Implementation Plan
- [`28-phase7-softfloat-plan.md`](design/28-phase7-softfloat-plan.md): Integer Spine: Milestone 15: Soft-Float Implementation Plan (phase 7, final roadmap phase)
- [`29-pic18-port-design.md`](design/29-pic18-port-design.md): 29: PIC18 port design
- [`30-distribution-design.md`](design/30-distribution-design.md): 30: Public binary distribution design
- [`31-ecosystem-integration-design.md`](design/31-ecosystem-integration-design.md): 31: epic-cc / epic-hal ecosystem integration design
- [`32-adding-a-device.md`](design/32-adding-a-device.md): 32: Adding a device (same core)
- [`33-pic14e-port-design.md`](design/33-pic14e-port-design.md): 33: PIC14E (Enhanced Mid-range) port design
- [`34-debugger-design.md`](design/34-debugger-design.md): 34: source-level debugger design
- [`35-sdcc-parity-design.md`](design/35-sdcc-parity-design.md): 35: SDCC parity epic
- [`36-pic18-access-bank-alloc-design.md`](design/36-pic18-access-bank-alloc-design.md): 36: PIC18 float-routine access-bank frame allocation
- [`36-robustness-test-strategy-design.md`](design/36-robustness-test-strategy-design.md): 36: Robustness test strategy, beyond differential fuzzing
- [`37-mutation-testing-triage.md`](design/37-mutation-testing-triage.md): 37: Lane C mutation-testing triage (docs/internals/design/36 section 2)
- [`37-pic-baseline-port-design.md`](design/37-pic-baseline-port-design.md): 37: PIC baseline core port design
- [`38-device-onboarding-hardening.md`](design/38-device-onboarding-hardening.md): 38: Device onboarding hardening: any 8-bit PIC part, one file
- [`39-common-ram-classification-design.md`](design/39-common-ram-classification-design.md): 39: common_ram classification for PIC14/PIC14E devices without a documented shared corner
- [`40-ecpp-subset-design.md`](design/40-ecpp-subset-design.md): 40: Embedded C++ subset design (PIC18 only)
- [`41-superopt-spike-findings.md`](design/41-superopt-spike-findings.md): 41: Superoptimizer spike findings (epic-cc#514)
- [`42-density-ceiling-findings.md`](design/42-density-ceiling-findings.md): 42: Density-ceiling findings (epic-cc#531)
- [`43-menu-triage-findings.md`](design/43-menu-triage-findings.md): 43: Menu-demo triage findings (epic-cc#617)
- [`44-code-factoring-spike-findings.md`](design/44-code-factoring-spike-findings.md): 44: Code-factoring spike findings (epic-cc#660)
- [`45-copy-coalescing-spike-findings.md`](design/45-copy-coalescing-spike-findings.md): 45: Copy-coalescing spike findings (epic-cc#664)
- [`46-public-beta-design.md`](design/46-public-beta-design.md): 46: Public beta through PlatformIO
- [`47-ram-overlay-findings.md`](design/47-ram-overlay-findings.md): 47: RAM overlay audit findings (epic-cc#728)
- [`48-ram-demo-audit-findings.md`](design/48-ram-demo-audit-findings.md): 48: RAM audit across the HAL demos (epic-cc#838)
- [`49-ram-gap-attribution.md`](design/49-ram-gap-attribution.md): 49: RAM gap attribution per demo (epic-cc#857)
- [`50-residual-scratch-park-survey.md`](design/50-residual-scratch-park-survey.md): 50: Residual scratch-slot park survey (epic-cc#1027)

## Design specs

- [`2026-08-21-cc4-inline-assembly-design.md`](specs/2026-08-21-cc4-inline-assembly-design.md): CC-4 Inline Assembly: Design
- [`2026-08-22-pic-variants-design.md`](specs/2026-08-22-pic-variants-design.md): PIC Variant Support -- file-per-device TOML + canonical-per-core CI -- Design
- [`2026-08-23-device-provenance-design.md`](specs/2026-08-23-device-provenance-design.md): Device data provenance -- generated TOML + an always-on gputils cross-check -- Design
- [`2026-08-24-const-table-select-design.md`](specs/2026-08-24-const-table-select-design.md): Runtime-indexed reads through ccp_sel-style pointer selects
- [`2026-08-24-indirect-calls-design.md`](specs/2026-08-24-indirect-calls-design.md): Indirect calls through function pointers -- Design
- [`2026-08-25-cross-context-callbacks-design.md`](specs/2026-08-25-cross-context-callbacks-design.md): Cross-context stored callbacks (store in main, invoke in the ISR) -- Design
- [`2026-08-25-max-min-abs-intrinsics-design.md`](specs/2026-08-25-max-min-abs-intrinsics-design.md): PIC14 signed min/max/abs intrinsics and s16 mul -- Design
- [`2026-08-25-runtime-inttoptr-design.md`](specs/2026-08-25-runtime-inttoptr-design.md): Runtime-address inttoptr lowering: FSR/INDF via slot-indirect
- [`2026-08-26-size-map-reporting-design.md`](specs/2026-08-26-size-map-reporting-design.md): Size and map reporting design
- [`2026-09-01-liveness-frame-overlay-design.md`](specs/2026-09-01-liveness-frame-overlay-design.md): Liveness-based intra-frame overlay design
- [`2026-09-22-pic18-exit-bank-carry-design.md`](specs/2026-09-22-pic18-exit-bank-carry-design.md): PIC18 interprocedural BSR exit-bank carry (design)
- `2026-09-22-xc8-size-parity-microbench-design.md`: moved to the private epic-benchmarks repo (see MIGRATION.md)
- [`2026-09-24-code-factoring-pass-design.md`](specs/2026-09-24-code-factoring-pass-design.md): PIC18 code-factoring pass (design)
