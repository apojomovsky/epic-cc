# 43: Menu-demo triage findings (epic-cc#617)

Top-down pass over the 1.31x menu-demo gap (epic-cc 11886 vs XC8 9068
words, 2026-09-23, both sim variant). Method: join our `--emit asm`
listing (per-function words from `scripts/density-profile.py`) with
XC8's per-function sizes (relinked `.map` symbol table, same sources)
on C symbol names. Naive per-function ratios lie wherever
`wholeprog_opt` folds single-caller callees: the table below merges
each folded callee into its caller (24 functions folded, from the
pre/post-`opt` `define` diff).

## Cluster-true worst offenders (ours vs XC8, words)

| cluster | ours | XC8 | ratio | gap |
|---|---|---|---|---|
| menu_demo_init (+CCP/ADC/LCD/gpio4/serial/tick inits) | 2171 | 1402 | 1.55 | 769 |
| main | 681 | 156 | 4.37 | 525 |
| redraw (+brightness/about inlined) | 669 | ~235 | ~2.8 | ~434 |
| epic_taskmgr_run (u32 tick loop) | 457 | 37 | 12.35 | 420 |
| menu_demo_task_heartbeat (+SetPWMDuty) | 496 | 154 | 3.22 | 342 |
| redraw_status | 441 | 105 | 4.20 | 336 |
| epic_dispatch_all_irqs (+5 IRQ handlers) | 396 | 239 | 1.66 | 157 |
| epic_serial_put_u16 (+put_udec) | 244 | 85 | 2.87 | 159 |
| gpio4_send | 546 | 291 | 1.88 | 255 |
| EPIC_USART_Init | 445 | 181 | 2.46 | 264 |
| task_stimulus (+push_event) | 266 | 76 | 3.50 | 190 |
| epic_taskmgr_on_timer0_overflow | 166 | 19 | 8.74 | 147 |
| epic_taskmgr_attach_timer0 | 242 | 141 | 1.72 | 101 |

The naive table (no merging) reads 11.6x on menu_demo_init and 27x on
put_u16; ~1200 of those words are inline attribution, not a gap. XC8
keeps single-caller callees outlined (own `.map` entries); we fold
them. The fold itself is flash-neutral per site (one body either
way) and buys the constant propagation `wholeprog_opt` exists for
(epic-cc#193); it only makes per-function triage lie, which this doc
corrects for.

## New micro benches (all in `size-bench/`, ladder + XC8 rows landed)

| bench | epic-cc | XC8 | gap | behind |
|---|---|---|---|---|
| bench-u32-loop | 105 / 23 | 49 / 9 | +56 | taskmgr_run 32-bit bound |
| bench-u16-dec | 140 / 39 | 126 / 17 | +14 | put_u16 decimal engine |
| bench-handle-init | 24 / 9 | 87 / 47 | -63 | (init cluster wins per-site) |
| bench-switch-calls | 50 / 10 | 63 / 3 | -13 | (redraw cluster wins per-site) |
## New profiler rules (`scripts/density-profile.py`)

- `store-reload-gap`: `MOVWF f`, 1-3 W-preserving `MOVFF`/`CLRF`
  moves avoiding `f`, then `MOVF f,W`. The near-miss half of #502.
  Menu-demo: 30 words.
- `wide-compare-branch`: 2+ `MOVF`/`SUBWF` lanes with per-lane
  `BNC`/`BZ` exits. The 32-bit loop bound and variable if-chains.
  Menu-demo: 150 words; bench-u32-loop: 20 of 105.
- `runtime-routine`: `__`-prefixed helper bodies plus `CALL`s to
  them (division and wide multiply lowering). Menu-demo: 254 words;
  startup (`__start`, `__epic_config`) and const pools keep their
  own attribution.

`other` moves 57.9% to 54.8%. The rest needs function-aware
profiling (inline clusters, call sequences, branch diamonds), which
the flat text matcher cannot see; filed as epic-cc#618.

## What this does not do (follow-ups)

- Callgraph-aware profiler attribution, filed as epic-cc#618 (merge
  folded callees the way this doc does by hand). Without it every
  future triage repeats this analysis and `other` stays above 50%.
- Optimization tickets for u32-loop (+56) and u16-dec (+14).
- A noinline/size-profitability study of the always-inline fold: kept
  as is here (flash-neutral per site), but unmeasured at whole-program
  scale against outlining + shared tails (XC8's `_put_u16_tail` shape).
