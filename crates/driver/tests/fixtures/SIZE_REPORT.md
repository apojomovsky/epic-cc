# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit; XC8 numbers quote the snapshot unless the oracle image was present.

- Date (UTC): 2026-09-24
- Commit: 3dec834
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- XC8 snapshot: measured 2026-09-24 (size-refresh-xc8: refreshed 16 benches)
- Menu-demo listing: 10914 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline | XC8 flash | XC8 RAM | gap | XC8 measured |
|---|---|---|---|---|---|---|---|---|
| bench-shift | 18F4550 | 90 | 26 | = / = | 108 | 16 | -18 | 2026-09-24 |
| bench-wide-const | 18F4550 | 23 | 10 | = / = | 28 | 6 | -5 | 2026-09-24 |
| bench-zero-init | 18F4550 | 25 | 16 | = / = | 38 | 12 | -13 | 2026-09-24 |
| bench-struct-copy | 18F4550 | 46 | 51 | = / = | 36 | 41 | +10 | 2026-09-24 |
| bench-switch | 18F4550 | 71 | 9 | = / = | 55 | 2 | +16 | 2026-09-24 |
| bench-bool | 18F4550 | 42 | 9 | = / = | 29 | 2 | +13 | 2026-09-24 |
| bench-dead-store | 18F4550 | 19 | 8 | = / = | 16 | 3 | +3 | 2026-09-24 |
| bench-w-roundtrip | 18F4550 | 21 | 10 | = / = | 16 | 3 | +5 | 2026-09-24 |
| bench-bank | 18F4550 | 33 | 15 | = / = | 30 | 10 | +3 | 2026-09-24 |
| bench-handle-init | 18F4550 | 24 | 9 | = / = | 87 | 47 | -63 | 2026-09-24 |
| bench-switch-calls | 18F4550 | 51 | 10 | = / = | 63 | 3 | -12 | 2026-09-24 |
| bench-u32-loop | 18F4550 | 86 | 23 | = / = | 49 | 9 | +37 | 2026-09-24 |
| bench-u16-dec | 18F4550 | 117 | 33 | = / = | 126 | 17 | -9 | 2026-09-24 |
| bench-struct-scan | 18F4550 | 148 | 99 | = / = | 88 | 96 | +60 | 2026-09-24 |
| bench-struct-scan | 16F877A | 177 | 108 | = / = | n/a | n/a | n/a | n/a (XC8 rows are 18F4550 only) |
| bench-bitmask | 18F4550 | 161 | 20 | = / = | 54 | 13 | +107 | 2026-09-24 |
| bench-hoist | 18F4550 | 135 | 18 | = / = | 137 | 13 | -2 | 2026-09-24 |

Negative gap means epic-cc is smaller. `vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline | XC8 flash | XC8 RAM | gap | XC8 provenance |
|---|---|---|---|---|---|---|---|---|
| add-16f877a | 16F877A | 12 | 9 | = / = | n/a | n/a | n/a | no XC8 counterpart |
| add-18f4550 | 18F4550 | 19 | 8 | = / = | n/a | n/a | n/a | no XC8 counterpart |
| hal-pic16-blink-16f877a | 16F877A | 614 | 47 | = / = | n/a | n/a | n/a | no XC8 counterpart |
| hal-pic18-blink-18f4550 | 18F4550 | 396 | 34 | = / = | n/a | n/a | n/a | no XC8 counterpart |
| hal-pic16-encoder-full-16f877a | 16F877A | 6909 | 329 | = / = | 5477 | 344 | +1432 | target slices, full set, generated config, example main |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 10992 | 755 | = / = | 8797 | 599 | +2195 | target variant, full peripheral set (file set differs from the sim-variant ladder entry) |

## Menu-demo clusters (listing words)

Driver total 10992 vs XC8 sim-variant 9068 (1.21x). Folded single-caller callees are rolled into their caller via `scripts/inline-map.py` (24 functions, the same merge `docs/43-menu-triage-findings.md` does by hand).

| cluster | ours | XC8 | ratio | gap |
|---|---|---|---|---|
| menu_demo_init (CCP/ADC/LCD/gpio4/serial/tick inits) +7 folded | 1945 | 1402 | 1.39 | +543 |
| main | 638 | 156 | 4.09 | +482 |
| redraw (brightness/about inlined) | 607 | ~235 | ~2.58 | ~+372 |
| gpio4_send | 543 | 291 | 1.87 | +252 |
| menu_demo_task_heartbeat (SetPWMDuty) +1 folded | 469 | 154 | 3.05 | +315 |
| epic_taskmgr_run (u32 tick loop) +1 folded | 447 | 37 | 12.08 | +410 |
| EPIC_USART_Init | 433 | 181 | 2.39 | +252 |
| redraw_status | 398 | 105 | 3.79 | +293 |
| epic_dispatch_all_irqs (5 IRQ handlers) +5 folded | 346 | 239 | 1.45 | +107 |
| task_stimulus (push_event) +1 folded | 257 | 76 | 3.38 | +181 |
| epic_taskmgr_attach_timer0 +2 folded | 208 | 141 | 1.48 | +67 |
| epic_serial_put_u16 (put_udec) | 196 | 85 | 2.31 | +111 |
| epic_taskmgr_on_timer0_overflow +1 folded | 124 | 19 | 6.53 | +105 |

Top profiler categories on the same listing:

- `other`: 5604 (51.3%)
- `struct-copy-movff`: 2074 (19.0%)
- `wide-literal-arith`: 895 (8.2%)
- `wide-const-materialization`: 598 (5.5%)
- `const-data`: 356 (3.3%)
- `sfr-context-save`: 308 (2.8%)
- `runtime-routine`: 236 (2.2%)
- `dead-store-reload`: 230 (2.1%)

## Regenerating

```
make size-report
```

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, refreshes the XC8 bench rows when the oracle image exists, and rewrites this file. XC8 whole-program and cluster rows stay snapshot quotes: they need different file sets and a manual `.map` join, so refreshing them is a deliberate act, not a side effect.
