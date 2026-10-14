# ADR-016: PIC18 fuzz gate (device-threaded differential runner)

**Status:** Accepted 2026-08-21 (implemented in feat/pic18-p8-fuzz)

## Decision

Point the differential fuzzer at PIC18 by threading a `&Device` through the harness:

* `crates/driver` gains `--device p18f4550|p16f877a` and `PIC8_DEVICE` env fallback, default `PIC16F877A`. The existing `match device.core` branch already selects the pipeline.
* `crates/fuzz` adds `device: &Device` to `run_differential`, `run_ir_differential`, `run_pic`, `pic_layout`, `run_ir_pic`. `run_pic` dispatches `Pic18` + `parse_hex_pic18` + 4096-byte RAM vs `Pic14` + 512-byte RAM; `run_ir_pic` dispatches `isel_pic18::select` + `asm::assemble_pic18` (no banking/peephole) vs the PIC14 `isel->banking->peephole` shape. `seed_le`/`read_le` become slice-based (RAM-size-agnostic). `driver_binary` cache is keyed by device.
* `crates/fuzz/tests/pic18.rs` provides the PIC18 gate: four fast subsets (integer/float/signed/IR seeds 0..8) as normal tests and three full corpora (integer 0..200, float 0..50, signed 0..50) as `#[ignore]` gates, mirroring the PIC14 gate shape. The fast gate is strict (every seed must be clean); the full gates must be clean with zero Mismatch/Panic/NoHalt/Compile/Harness.

PIC14 behaviour is preserved: every existing call site passes `&PIC16F877A`, so the fast PIC14 corpora stay green unmodified and remain the regression gate.

## Rationale

The seed corpora are the port's correctness net: every IR shape the generator can emit is an isel-pic18 obligation. Running the same seeded C through host clang and through the PIC18 pipeline (driver + Pic18 sim) and comparing checksums exercises the whole PIC18 stack (legalize, alloc, isel-pic18, asm, sim) without new oracles. The msp430 datalayout proxy carries over (8-bit char, 16-bit int/pointers), so the generators stay device-independent; `host_main_source` seeds by name, so the PIC18 alloc layout is consumed consistently via `pic_layout`.

Gaps surfaced by the corpus are fixed in isel-pic18 in the same branch, minimally and with a panic string, not papered over in the harness.

## Rejected alternatives

* Duplicating the harness for PIC18: two copies of the same generator/runner logic, drift risk.
* Env-only device selection with no param: every differential call would need to set and restore env, racy in parallel tests.
* Hard-coding PIC18 in a separate fuzz crate: loses the shared generator and host runner.

## Revisit if

A third core arrives: the `device.core` dispatch generalizes, but a `Device`-keyed OnceLock cache may need a single binary per core model.
