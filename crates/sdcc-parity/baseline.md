# SDCC parity baseline (P0, epic-cc#270)

The honest starting table: epic-cc vs SDCC 4.6.0 on the committed corpus,
measured 2026-09-07. This is the anchor for "when we're done" (docs/35
section 5). Regenerate with `scripts/sdcc-parity.sh` in the dev image.

SDCC 4.6.0, gputils 1.5.2. epic-cc at the commit this file ships with.

Legend: `PASS` = both compilers accepted the program and the named outputs
matched. `MISMATCH` = both accepted but outputs differed. `GAP` = epic-cc
cannot compile it yet (surface gap, tracked by the sub-epics). `PANIC` = the
sim aborted on SDCC's output (a real finding). `SDCC-ERR` = SDCC rejected the
program.

| Program | Device | epic flash | sdcc flash | epic RAM | sdcc RAM | epic cyc | sdcc cyc | Result |
|---|---|---|---|---|---|---|---|---|
| add | p16f877a | 13 | 158 | 512 | 512 | 13 | 5000000 | PASS |
| add | p18f4550 | 13 | 7 | 4096 | 4096 | 9 | 5000000 | PASS |
| array | p16f877a | 50 | 158 | 512 | 512 | 49 | 5000000 | PASS |
| array | p18f4550 | 48 | 15 | 4096 | 4096 | 40 | 5000000 | PASS |
| bitfields | p16f877a | 26 | 158 | 512 | 512 | 26 | 5000000 | PASS |
| bitfields | p18f4550 | 39 | 32 | 4096 | 4096 | 30 | 5000000 | PASS |
| union | p16f877a | 11 | 158 | 512 | 512 | 11 | 5000000 | PASS |
| union | p18f4550 | 10 | 3 | 4096 | 4096 | 6 | 5000000 | PASS |
| i64 | p16f877a | 51 | 158 | 512 | 512 | 115 | 5000000 | PASS |
| i64 | p18f4550 | 26 | 13 | 4096 | 4096 | 18 | 5000000 | PASS |
| double | p16f877a | - | - | - | - | - | - | GAP (epic-cc: unsupported type double) |
| double | p18f4550 | - | - | - | - | - | - | GAP (epic-cc: unsupported type double) |
| malloc | p16f877a | 9 | 158 | 512 | 512 | 9 | 5000000 | PASS |
| malloc | p18f4550 | 10 | 3 | 4096 | 4096 | 6 | 5000000 | PASS |
| math | p16f877a | 14 | 158 | 512 | 512 | 13 | 5000000 | PASS |
| math | p18f4550 | 14 | 3 | 4096 | 4096 | 9 | 5000000 | PASS |
| fnptr | p16f877a | 81 | 158 | 512 | 512 | 501 | 5000000 | PASS |
| fnptr | p18f4550 | 57 | 3 | 4096 | 4096 | 119 | 5000000 | PASS |
| recursion | p16f877a | - | - | - | - | - | - | SDCC-ERR (pic14: invalid combination of short/long) |
| recursion | p18f4550 | - | - | - | - | - | - | MISMATCH (epic=0x78 sdcc=0x1) |
| printf-f | p16f877a | - | - | - | - | - | - | MISMATCH (epic=0xa sdcc=0xef) |
| printf-f | p18f4550 | - | - | - | - | - | - | PANIC (sim: index out of bounds 65535) |

## Findings

- **SDCC does not halt.** Its linked output loops after `main` returns, so
  the sim runs to the 5M-step budget and reads the named outputs. Cycle
  counts for SDCC are therefore the budget, not a real measure; the size
  comparison (flash/RAM) is the meaningful axis until SDCC's startup is
  understood.
- **SDCC flash counts are small** (7-32 words on PIC18) because the gplink
  map's section info only counts the program's own code, not the linked
  startup/lib. The comparison is apples-to-oranges until this is reconciled.
- **Output mismatches** (recursion, printf-f on PIC18) are real differential
  findings: both compilers accepted the program but produced different
  results. These are the gaps the sub-epics must close.
- **PANIC on PIC18** (printf-f): SDCC's output accesses address 0xFFFF,
  which our sim does not model (likely a config/EEPROM access). Real finding.
- **`double` is a surface gap** on epic-cc (unsupported type), tracked by the
  PIC18 sub-epic.
- **SDCC pic14 rejects `double`** (error 206: invalid combination of
  short/long), so the double probe is PIC18-only in practice.
