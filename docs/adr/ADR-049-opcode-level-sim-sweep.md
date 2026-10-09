# ADR-049 -- Opcode-level MPLAB SIM sweep in four shards

**Status:** Accepted 2026-10-09<br>
**Decides:** `epic-cc#819`<br>
**Evidence:** `crates/superopt/src/sweep.rs` plus its builder tests;
`pic18-arith`, `pic18-logic`, `pic14-arith`, `pic14-logic` in
`.github/workflows/mdb-oracle.yml`

## Context

The `SUBFWB`/`SUBWFB` mixup lived because the sim and the compiler
shared one wrong belief about an opcode, and every sim-backed gate
agreed with the wrong assembly. Only the external MDB gate could see
it, and that gate replayed whole superopt sequences, never opcodes in
isolation across carry and borrow states.

## Decision

- **One lane per ALU opcode, four shards.** Each lane replays a single
  instruction across entry-`W`, operand, and entry-`STATUS` corners;
  shards split PIC18 arithmetic, PIC18 logic and rotates, and PIC14 in
  two, so CI wall time stays flat.
- **Lane-tagged mismatch logs.** Every read carries its lane, and the
  diff reports all mismatches grouped by lane, so one log names each
  failing opcode without re-running.
- **PIC14 stores banked, STATUS masked in-program.** The table spans
  banks 0-3 through RP1:RP0 selects; the captured `STATUS` clears TO/PD
  on both executors, since SIM reports POR-or-WDT values there
  nondeterministically; the park loop pets the watchdog.
- **PR tier on oracle inputs only**, same policy as the sequence gate:
  `crates/sim`, `crates/asm`, `crates/superopt`, the oracle script and
  workflow; wider corners ride the nightly sweep.

## What the hardware taught (amendment 2026-10-09)

Running the sweep convicted MPLAB SIM 6.35 itself on three flag
models, each hand-verified instruction by instruction against boolean
truth and triangulated with gpsim 0.31 (the in-tree sim matches truth
on all 15 probe points): ADDWFC drops the (W+C) low-nibble carry from
DC, SUBWFB and SUBFWB miscompute DC+OV with borrow-in, and DAW tests
the original high nibble (wrong for valid BCD sums like 0x99+0x06,
which must yield 0x05 with C set). The three STATUS bits are masked
per lane in-program on both executors, DAW has no lane, and the
masked behavior stays pinned by in-tree sim tests. Remainder in
epic-cc#1001.

## Rejected alternatives

- **One spec per opcode (44 matrix jobs).** Free attribution but 44
  image pulls and job slots per PR; the account's concurrency is shared
  with the other repos.
- **One job per core.** Two jobs hide which opcode failed and serialize
  every chunk behind one runner.
- **Skipping literal-`k` and rotate corners.** The borrow confusion hid
  exactly in `SUBWFB` versus `SUBFWB`; untested corners are where the
  next shared belief lives.

## Revisit if

A sweep lane fails on silicon behavior the datasheet confirms (the sim,
not the lane, is then wrong), or shard wall time exceeds the sequence
gate and needs finer chunking.
