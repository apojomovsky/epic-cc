# 50: Residual scratch-slot park survey (epic-cc#1027)

Survey of the `MOVWF 0x70` park sites that remain in the 16F877A `-Os`
listing of `hal-pic16-encoder-full` after #751, #1024 and #1066. No
codegen change. The output ranks candidate rewrites by an upper bound
on words recoverable. Each candidate still needs a proof before it is
implemented; implementations are follow-ups.

This is a dated survey snapshot, measured on `d80e48d`. Counts below
describe that listing and will go stale on the next codegen change.

Measured with the `size_regression_e2e` input list for
`hal-pic16-encoder-full-16f877a`, default `-Os`, `--emit asm`:

| measure | count |
|---|---|
| instructions in listing | 5233 |
| `MOVWF 0x70` (exact operand) | 162 |
| any instruction naming file `0x70` | 337 |
| park sites (defined below) | 151 |
| `MOVWF 0x70` not parked (control leaves the block first) | 11 |

The ticket's "about 162" matches the exact-operand count. A broader
count that matches any `0x70` operand gives 337. No pattern measured
here reproduces the 814 figure from an earlier grep.

## Park site definition

A park site is a `MOVWF 0x70` whose stored W is read back from `0x70`
on the straight-line path before any write to `0x70`, and before any
`CALL`, `GOTO`, `RETLW`, `RETURN` or `RETFIE`. The scan covers at most
60 listing instructions. Skip instructions (`BTFSC`, `BTFSS`, `INCFSZ`,
`DECFSZ`) on other operands do not end the scan. The read may be a
restore (`MOVF 0x70,W`) or an operand use (`ADDWF`, `IORWF`, `SUBWF`,
`INCFSZ` with `W` destination).

The 11 non-park sites are all control-terminated: 10 precede a `CALL`,
whose callee is not inspected here, and 1 precedes `RETFIE` in an ISR.

## Producer and consumer

Producer: the instruction that wrote W before the park. Consumer: the
instruction that reads `0x70` after the park.

| producer | consumer | sites |
|---|---|---|
| `MOVLW` const | `INCFSZ 0x70,W` | 38 |
| `ADDWF 0x70,W` | `ADDWF 0x70,W` | 28 |
| `MOVLW 0x00` | `ADDWF 0x70,W` | 10 |
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

Consumer families:

- `INCFSZ 0x70,W` (38): the stored constant is `MOVLW 0x00` at 28
  sites and `MOVLW` 0xFF, 0xFE, 0x13 or 0xEC at the other 10. Only 15
  of the 38 have the carry shape
  `BTFSC STATUS,0 / INCFSZ 0x70,W / ADDWF hi,F`. The other 23 are not
  classified further.
- `ADDWF 0x70,W` (38): each read is followed directly by `MOVWF 0x70`,
  so the accumulator is threaded through the park. The first operand
  load is `ADDWF 0x70,W` (28, chained) or `MOVLW 0x00` (10).
- `IORWF`/`SUBWF 0x70,W` (44): mask and subtract operands produced by
  `XORWF`, `XORLW`, `ANDLW` or `MOVF`.
- `MOVF 0x70,W` after `MOVF` (19): spill of the first operand around a
  second load into W, then restored.
- `MOVF 0x70,W` after `RETLW` (12): table-read value parked across a
  `MOVLW HIGH(table) / MOVWF PCLATH` page switch, then `ADDLW LOW`.
  Same family as #1024.

The `__read_*` symbols occur within 8 listing instructions of 32 park
sites (routines: `__read_irq_table`, `__read_compute_period.pre_ratio`,
`__read_compute_period.pre_enum`).

## Ranking

Upper bound per site: a pure restore frees the park word and the
restore word, so 2 words. An operand consumer keeps its operand word
(the `0x70` file operand is still needed), so 1 word, the park. Both
bounds assume W can be kept or reordered without changing behavior,
which is not proven for any class.

| rank | class | sites | upper bound (words) | what must be proven |
|---|---|---|---|---|
| 1 | `IORWF`/`SUBWF 0x70,W` operand | 44 | 44 | operand reachable without the park, producer flags kept |
| 2 | `INCFSZ 0x70,W` constant operand | 38 | 38 | non-zero constants (10 sites) and the zero constant (28) both replaceable without a file slot; carry and `Z` identical |
| 3 | `ADDWF 0x70,W` accumulate | 38 | 38 | sum order and carry chain identical; `ADDWF` sets `C`/`DC`/`Z` per add, so a fused add must set the same flags |
| 4 | `MOVF 0x70,W` after `MOVF` spill | 19 | 38 | reorder of the second load into W is hazard-free |
| 5 | `MOVF 0x70,W` after `RETLW` PCLATH switch | 12 | 24 | index consumable without a W-clobbering page write; same constraint as #1024 |

Totals: 182 words upper bound over 151 sites. Size and cycle effects
are not measured. Each class needs its own proof and its own size and
cycle check against `size_baseline.toml` and `cycle_baseline.toml`.

## Out of scope

Each candidate is a follow-up with `blockedBy` on this survey. None
changes codegen here. Rank 3 is the one the flag caveat applies to
most directly: the flags a consumer reads after each `ADDWF` must be
checked before any fused form.
