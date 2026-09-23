# ADR-039 -- Zero-initialized RAM is cleared by __start on PIC18

Status: Accepted 2026-09-23<br>
Decides: epic-cc#561

## Context

RAM-resident globals without an initializer read whatever RAM powers up
with. The simulator zero-fills RAM, so the reliance was invisible there;
real PIC RAM is indeterminate at power-on, so a `static unsigned char
buf[16]` read before any write is zero in-sim and arbitrary on hardware.
Measurement on the menu-demo fixture (epic-cc#469 ladder case): 42
zero-initialized globals, 458 bytes of RAM.

Three options were priced:

- Clear only never-stored globals. 18 globals (334 bytes) are never
  stored anywhere in the program; the rest need read-before-write
  analysis the compiler does not have. Saves little over clearing
  everything (73% of the bytes need it anyway) and leaves a residual
  reliance plus analysis debt.
- Clearing loop over every zero-init run. The #486 POSTINC loop shape
  (LFSR seed, count in WREG, CLRF POSTINC0, DECFSZ, BRA) costs 6 words
  per run with no scratch byte and no MOVLB traffic. Menu-demo needs 6
  runs: 36 words, +0.3% flash. No dataflow analysis, no residual class.
- Document and keep. Rejected: C requires zero-initialized statics, and
  no evidence shows the target guarantees it.

## Decision

- `__start` clears every zero-initialized RAM global with one
  LFSR-seeded CLRF loop per contiguous run (runs join adjacent
  zero-init globals, split at 255 for the one-byte count), before the
  nonzero init stores. PIC18 only; the other cores keep the old
  reliance until measured the same way.
- Uniform loop for every run length: a second straight-CLRF path would
  save ~10 words on menu-demo for a whole emission branch.
- Test harnesses drive runtime inputs through real initializers (or
  `-D` per value), never through sim-side seeds into uninitialized
  globals: seeds do not survive the clear, by design.

## Consequences

- `zero_ram_e2e` pins the guarantee: a non-zero power-on seed reads
  back 0.
- Size ladder re-baselined (menu-demo 12137 to 12056 words: the
  clearing loops cost 36, other drift is unrelated).
- PIC14/PIC14E/baseline still rely on zeroed RAM: follow-up work.
