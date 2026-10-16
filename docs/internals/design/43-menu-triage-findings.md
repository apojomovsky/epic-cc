# 43: Menu-demo triage findings (epic-cc#617)

Top-down pass over the menu-demo listing (epic-cc 11886 words,
2026-09-23, sim variant). Method: per-function words from
`scripts/density-profile.py` over our `--emit asm` listing. Naive
per-function numbers lie wherever `wholeprog_opt` folds single-caller
callees: the table below merges each folded callee into its caller
(24 functions folded, from the pre/post-`opt` `define` diff). The
comparison against a reference compiler that drove the ranking is kept
in the private epic-benchmarks repo (ADR-006).

## Largest clusters (words)

| cluster | ours |
|---|---|
| menu_demo_init (+CCP/ADC/LCD/gpio4/serial/tick inits) | 2171 |
| main | 681 |
| redraw (+brightness/about inlined) | 669 |
| gpio4_send | 546 |
| menu_demo_task_heartbeat (+SetPWMDuty) | 496 |
| epic_taskmgr_run (u32 tick loop) | 457 |
| EPIC_USART_Init | 445 |
| redraw_status | 441 |
| epic_dispatch_all_irqs (+5 IRQ handlers) | 396 |
| task_stimulus (+push_event) | 266 |
| epic_serial_put_u16 (+put_udec) | 244 |
| epic_taskmgr_attach_timer0 | 242 |
| epic_taskmgr_on_timer0_overflow | 166 |

The fold is flash-neutral per site (one body either way) and buys the
constant propagation `wholeprog_opt` exists for (epic-cc#193); it only
makes per-function triage lie, which this doc corrects for.

## New micro benches (all in `size-bench/`, ladder rows landed)

| bench | epic-cc flash / RAM | behind |
|---|---|---|
| bench-u32-loop | 105 / 23 | taskmgr_run 32-bit bound |
| bench-u16-dec | 140 / 39 | put_u16 decimal engine |
| bench-handle-init | 24 / 9 | init cluster, per-site shape |
| bench-switch-calls | 50 / 10 | redraw cluster, per-site shape |

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
- Optimization tickets for u32-loop and u16-dec.
- A noinline/size-profitability study of the always-inline fold: kept
  as is here (flash-neutral per site), but unmeasured at whole-program
  scale against outlining and shared tails.
