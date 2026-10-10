# 50: Residual scratch-slot park survey (epic-cc#1027)

Survey of the `MOVWF 0x70` park sites that remain in the 16F877A `-Os`
listing of `hal-pic16-encoder-full` after #751, #1024 and #1066. No
codegen change. The output ranks candidate rewrites by an upper bound
on words recoverable; each candidate still needs a proof before it is
implemented, and the implementations are follow-ups.

Measured on `d80e48d`, with the `size_regression_e2e` input list for
`hal-pic16-encoder-full-16f877a`, default `-Os`, `--emit asm`:

| measure | count |
|---|---|
| instructions in listing | 5233 |
| `MOVWF 0x70` (exact operand) | 162 |
| any instruction naming file `0x70` | 337 |
| park sites (defined below) | 151 |
| `MOVWF 0x70` not parked (control leaves the block first) | 11 |

The ticket's "about 162" matches the exact-operand count. A broader
count that matches any `0x70` operand gives 337, not the 814 figure
seen in an earlier grep; no pattern measured here reproduces 814.

## Park site definition

A park site is a `MOVWF 0x70` whose stored W is read back from `0x70`
on the straight-line path before any write to `0x70` and before any
`CALL`, `GOTO`, `RETLW` or `RETFIE`. Skip instructions (`BTFSC`,
`BTFSS`, `INCFSZ`, `DECFSZ`) on other operands do not end the scan.
The read may be a restore (`MOVF 0x70,W`) or an operand use
(`ADDWF`, `IORWF`, `SUBWF`, `INCFSZ` with `W` destination).

The 11 non-park sites all sit in an ISR or call epilogue, so the
parked value is consumed by a caller or ISR return, not by this block.

## Producer and consumer

Producer: the instruction that wrote W before the park. Consumer: the
instruction that reads `0x70` after the park.

| producer | consumer | sites |
|---|---|---|
| `MOVLW` const | `INCFSZ 0x70,W` | 38 |
| `ADDWF` | `ADDWF 0x70,W` | 28 |
| `MOVLW` const | `ADDWF 0x70,W` | 10 |
| `MOVF` | `IORWF 0x70,W` | 11 |
| `IORWF` | `IORWF 0x70,W` | 14 |
| `XORWF` | `IORWF 0x70,W` | 8 |
| `XORLW` | `IORWF 0x70,W` | 7 |
| `ANDLW` | `IORWF 0x70,W` | 1 |
| `XORWF` | `SUBWF 0x70,W` | 3 |
| `MOVF` | `MOVF 0x70,W` | 19 |
| `RETLW` | `MOVF 0x70,W` | 12 |

Totals: 120 sites are operand consumers (the `0x70` file register is
used as an operand); 31 are pure restores (`MOVF 0x70,W`).

Consumer kinds in the table above by family:

- `INCFSZ 0x70,W` (38): 16-bit carry idiom,
  `BTFSC STATUS,0 / INCFSZ 0x70,W / ADDWF hi,F`, with `0x70` holding a
  constant zero stored by `MOVLW 0 / MOVWF 0x70`.
- `ADDWF 0x70,W` (38): running accumulate `MOVF src,W / ADDWF 0x70,W /
  MOVWF 0x70`, where the first operand is a constant or a file register.
- `IORWF`/`SUBWF 0x70,W` (44): mask and subtract operands produced by
  `XORWF`, `XORLW`, `ANDLW` or `MOVF`.
- `MOVF 0x70,W` after `MOVF src,W` (19): spill of the first operand
  around a second load into W, then restored.
- `MOVF 0x70,W` after `RETLW` (12): table-read value parked across a
  `MOVLW HIGH(table) / MOVWF PCLATH` page switch, then `ADDLW LOW`.
  Same family as #1024.

The `__read_*` quad the ticket describes accounts for 32 of the 151
sites by proximity to a `__read_*` call or label within 8 lines.

## Ranking

Upper bound per site: a pure restore frees the park word and the
restore word, so 2 words. An operand consumer keeps its operand word
(the `0x70` file operand is still needed), so 1 word, the park. Both
bounds assume W can be kept or reordered without changing behavior,
which is not proven for any class.

| rank | class | sites | upper bound (words) | what must be proven |
|---|---|---|---|---|
| 1 | `IORWF`/`SUBWF 0x70,W` operand | 44 | 44 | operand reachable without the park, flags of the producer kept |
| 2 | `INCFSZ 0x70,W` carry idiom | 38 | 38 | constant-zero operand equivalent without a file slot, carry and `Z` identical |
| 3 | `ADDWF 0x70,W` accumulate | 38 | 38 | sum order and carry chain identical; `ADDWF` sets `C`/`DC`/`Z` per add, so a fused add must set the same flags |
| 4 | `MOVF 0x70,W` after `MOVF` spill | 19 | 38 | reorder of the second load into W is hazard-free |
| 5 | `MOVF 0x70,W` after `RETLW` PCLATH switch | 12 | 24 | index consumable without a W-clobbering page write; same constraint as #1024 |

Totals: 182 words upper bound, 151 sites. Not measured; each class needs
its own proof and its own size and cycle check against
`size_baseline.toml` and `cycle_baseline.toml`.

## Out of scope

Each candidate is a follow-up with `blockedBy` on this survey. None
changes codegen here. Rank 3 is the one the flag caveat applies to
most directly: a 5-add chain and one add do not leave the same
`C`/`DC`/`Z`, so the flags the consumer reads must be checked before any
fused form.
