# SDCC parity baseline (P0, epic-cc#266)

The honest starting table: epic-cc vs SDCC 4.6.0 on the committed corpus,
regenerated under the P0 comparison protocol (docs/35 section 4). This is
the anchor for "when we're done" (docs/35 section 5). Regenerate with
`scripts/sdcc-parity.sh` in the dev image; the machine-readable rows live
in `crates/sdcc-parity/baseline.toml` (regenerated with
`UPDATE_SDCC_BASELINE=1 cargo test -p sdcc-parity --test regression_gate`)
and a ratio regression against them fails CI.

versions: epic-cc 0.1.0; SDCC : pic16/pic14 TD- 4.6.0 #16555 (Linux); gplink-1.5.2 #1325 (Sep  7 2026)

Legend: `PASS` = both compilers accepted the program, both ran to the
`sleep` halt in our simulator, and the named outputs matched.
`SDCC-BUG` = epic-cc matches the hand-computed expected value and SDCC
does not (arbitrated in `sdcc-known-bugs.toml`, each entry cross-checked
under gpsim). `SDCC-LIMIT` = SDCC cannot compile the program at all on
that core. Flash = whole-image program words from each side's final HEX;
RAM = live (non-zero) RAM bytes at the halt; cycles = our simulator's
cycle count to the `sleep` halt, one definition for both compilers.

| Program | Device | epic flash | sdcc flash | epic RAM | sdcc RAM | epic cyc | sdcc cyc | Result |
|---|---|---|---|---|---|---|---|---|
| add | p16f877a | 18 | 235 | 4 | 12 | 16 | 322 | PASS |
| add | p18f4550 | 16 | 106 | 6 | 17 | 10 | 6217 | PASS |
| add | p16f1938 | 17 | 198 | 4 | 16 | 15 | 281 | PASS |
| array | p16f877a | 41 | 283 | 7 | 19 | 39 | 366 | PASS |
| array | p18f4550 | 44 | 133 | 10 | 21 | 33 | 6238 | PASS |
| array | p16f1938 | 48 | 228 | 10 | 21 | 44 | 309 | PASS |
| bitfields | p16f877a | 31 | 431 | 6 | 21 | 29 | 512 | PASS |
| bitfields | p18f4550 | 32 | 207 | 8 | 22 | 24 | 6305 | PASS |
| bitfields | p16f1938 | 30 | 316 | 6 | 26 | 28 | 399 | PASS |
| unions | p16f877a | 57 | 253 | 8 | 16 | 54 | 340 | PASS |
| unions | p18f4550 | 53 | 117 | 10 | 17 | 43 | 6228 | PASS |
| unions | p16f1938 | 56 | 211 | 10 | 20 | 43 | 294 | PASS |
| i64 | p16f877a | - | - | - | - | - | - | SDCC-LIMIT (error 206: no 64-bit on pic14; epic computes 0x9A) |
| i64 | p18f4550 | 16 | 106 | 7 | 18 | 10 | 6216 | PASS |
| i64 | p16f1938 | - | - | - | - | - | - | SDCC-LIMIT (error 206: no 64-bit on pic14; epic computes 0x9A) |
| double | p16f877a | 648 | 2506 | 14 | 54 | 1093 | 8748 | PASS |
| double | p18f4550 | 728 | 1409 | 18 | 43 | 1057 | 8080 | PASS |
| double | p16f1938 | 644 | 1947 | 14 | 53 | 1089 | 6670 | PASS |
| malloc | p16f877a | 13 | 260 | 1 | 13 | 11 | 354 | PASS |
| malloc | p18f4550 | - | - | - | - | - | - | SDCC-BUG (epic=0x5A sdcc=0xF6; SDCC generic-pointer dereference, gpsim-confirmed) |
| malloc | p16f1938 | 12 | 220 | 1 | 14 | 10 | 311 | PASS |
| math | p16f877a | 58 | 557 | 10 | 26 | 120 | 1006 | PASS |
| math | p18f4550 | 29 | 112 | 11 | 19 | 19 | 6222 | PASS |
| math | p16f1938 | 56 | 433 | 10 | 29 | 118 | 791 | PASS |
| fnptr | p16f877a | - | - | - | - | - | - | SDCC-BUG (epic=0x0A sdcc=0xF4; SDCC pic14 computed call, gpsim-confirmed) |
| fnptr | p18f4550 | 17 | 146 | 6 | 23 | 10 | 6247 | PASS |
| fnptr | p16f1938 | - | - | - | - | - | - | SDCC-BUG (epic=0x0A sdcc=0xCF; SDCC pic14 computed call, gpsim-confirmed) |
| recursion | p16f877a | - | - | - | - | - | - | SDCC-BUG (epic=0x78 sdcc=0x1; SDCC static-overlay recursion, gpsim-confirmed) |
| recursion | p18f4550 | 60 | 145 | 14 | 42 | 120 | 6333 | PASS |
| recursion | p16f1938 | - | - | - | - | - | - | SDCC-BUG (epic=0x78 sdcc=0x1; SDCC static-overlay recursion, gpsim-confirmed) |
| printf-f | p16f877a | 14 | 231 | 3 | 11 | 12 | 318 | PASS (probe is a placeholder; the real %f probe is the PIC18 sub-epic's) |
| printf-f | p18f4550 | 13 | 102 | 5 | 18 | 7 | 6212 | PASS (placeholder, as above) |
| printf-f | p16f1938 | 13 | 195 | 3 | 15 | 11 | 278 | PASS (placeholder, as above) |

aggregate over 26 comparable rows: flash ratio 0.151, RAM ratio 0.301,
cycle ratio 0.027 (epic-cc/SDCC, geometric mean).

## Findings

- **Self-seeding corpus.** SDCC's PIC18 crt0 clears all of BSS before
  `main` (an FSR0 walk from the top of SRAM down to 0x000), so a value
  written into RAM before the run never survives to the program. The
  first baseline's PIC18 rows were poisoned by exactly this: `out = in + 1`
  with a wiped seed reported `0 + 1`. The protocol now assigns inputs at
  the top of `main`; nothing is seeded from outside.
- **Halt protocol.** Every corpus program ends in `__asm__("sleep")`, the
  simulator's halt condition, so SDCC's never-returning post-`main` idle
  loop no longer inflates its cycle counts to the step budget. SDCC's
  PIC18 cycles carry its ~6K-instruction BSS-clear + `cinit` startup,
  which is the honest whole-program cost.
- **Three simulator gaps found by the oracle, now fixed.** The first
  PIC18 rows recorded SDCC as wrong on almost every program; gpsim
  (independent reference) proved the wrong side was ours. The sim did
  not model WREG as access-bank file register 0xFE8 (every chained
  `RLNCF WREG, W` sequence read zero), did not model `MOVWF PCL`
  computed jumps (SDCC's `__sdcc_call`), did not route TOS-register
  writes into the hardware stack (SDCC plants return addresses that
  way), and resolved virtual-register operands twice in d=1
  read-modify-write ops (`MOVF POSTINC1, F` popped twice). All four are
  fixed and the rows they poisoned now pass.
- **Genuine SDCC bugs remain arbitrated** in `sdcc-known-bugs.toml`,
  each verified under gpsim as well as our sim: SDCC's PIC18
  generic-pointer dereference (malloc), its PIC14-family computed-call
  bug (fnptr), and its PIC14-family static-overlay recursion corruption
  (recursion, the manual-documented no-hardware-stack limitation).
- **epic-cc leads on all three axes** across the comparable corpus:
  geometric-mean flash ratio 0.151, RAM ratio 0.301, cycle ratio 0.027.
  The double probe (soft-float) is the heaviest row and still under
  SDCC everywhere it runs.
