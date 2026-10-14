# ADR-044 -- PIC18 borrow chains use `SUBWFB`, never `SUBFWB`

**Status:** Accepted 2026-09-30<br>
**Decides:** `epic-cc#632`<br>
**Evidence:** hardware probe on `18F4550` (gpasm 1.5.2 byte encodings plus
MPLAB SIM readback), `crates/driver/tests/usart_param_spbrg_e2e.rs`,
`epic-hal/scripts/compare-toolchains.sh epic-menu-demo`

## Context

PIC18 has two subtract-with-borrow file ops, and they are not variants of
one operation:

- `SUBWF f,d` is `f - W`.
- `SUBWFB f,d` is `f - W - !C` (borrow-in).
- `SUBFWB f,d` is `W - f - !C`: the operands reversed.

`isel-pic18` emitted `SUBFWB` for every byte above the first of a multi-byte
subtraction, in the IR `Sub` path and in the runtime `__udiv_u32` restoring
divide's `rem -= den` chain. Both need `f - W - borrow` with the remainder
or destination as `f`, so the reversed form computed
`W - f - borrow`: a wrong result on every multi-byte subtraction and every
runtime divide whenever a borrow crossed a byte boundary. Constants folded
before isel hid it in the folded shapes, which is why the control-demo gate
stayed green while `epic-menu-demo` did not.

The defect survived because the simulator decoded both opcodes with one
formula. Every sim gate, including the whole-program ones, agreed with the
wrong asm; only MPLAB SIM and real silicon disagreed, so the bug was
invisible to everything except the external gate.

## Decision

- **Borrow chains emit `SUBWFB`.** The IR `Sub` carry lanes and the runtime
  divide's subtract step use `SUBWFB`; `SUBWF` stays for the carry-free
  first byte or lane, and the const-LHS path keeps `SUBLW`.
- **The simulator models the two opcodes separately.** `0x5400` decodes as
  `W + !f + C`, `0x5800` as `f + !W + C`. A shared decode is what let a
  wrong mnemonic pass every in-tree gate.
- **The regression test drives the runtime path.** The fixture forwards
  runtime `fosc_hz`/`baud` through an inner call, the `epic_serial_init`
  shape, so whole-program constant folding cannot remove the divide, and it
  asserts the `SPBRG:SPBRGH` result the mdb gate reads.

## Rationale

**Hardware over the local oracle.** The two mnemonics were distinguished by
assembling both with gpasm and reading the results back through MPLAB SIM
(`W - f` versus `f - W` under the same inputs), not by reading the
datasheet alone or trusting the in-tree simulator. That is the only
source that can arbitrate an encoder/decoder disagreement.

**Fold-proof fixture.** The bug's blast radius was masked by constant
folding, so a regression test that folds what it checks reproduces
nothing. Laundering the constants through a zeroed volatile keeps the
divide in the emitted code.

## Consequences

- The simulator's opcode coverage is now load-bearing: a wrong mnemonic in
  emitted asm is caught in-tree, where before only the external mdb gate
  could see it. Any future opcode added to both a decoder and an emitter
  needs its two directions checked against gpasm, not just made to run.
- `epic-menu-demo`'s epic-cc leg produces a full session with
  `EPIC_HARNESS_RESULT: PASS`; its SPBRG reads 1249 (`0x04E1`) under MPLAB
  SIM where it read the `0xFFFF` sentinel. The remaining XC8/epic-cc UART
  divergence is the fired-stimulus tick values only (`0005 000A 000F 0014
  0019` versus XC8's `0005 001A 001A 001A 001A`), a timing difference the
  harness already documents as non-asserted, filed separately.
