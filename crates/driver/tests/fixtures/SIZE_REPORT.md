# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit.

- Date (UTC): 2026-09-24
- Commit: 3b425e8
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- Menu-demo listing: 10914 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| bench-shift | 18F4550 | 90 | 26 | = / = |
| bench-wide-const | 18F4550 | 23 | 10 | = / = |
| bench-zero-init | 18F4550 | 25 | 16 | = / = |
| bench-struct-copy | 18F4550 | 46 | 51 | = / = |
| bench-switch | 18F4550 | 71 | 9 | = / = |
| bench-bool | 18F4550 | 42 | 9 | = / = |
| bench-dead-store | 18F4550 | 19 | 8 | = / = |
| bench-w-roundtrip | 18F4550 | 21 | 10 | = / = |
| bench-bank | 18F4550 | 33 | 15 | = / = |
| bench-handle-init | 18F4550 | 24 | 9 | = / = |
| bench-switch-calls | 18F4550 | 51 | 10 | = / = |
| bench-u32-loop | 18F4550 | 86 | 23 | = / = |
| bench-u16-dec | 18F4550 | 117 | 33 | = / = |
| bench-struct-scan | 18F4550 | 148 | 99 | = / = |
| bench-struct-scan | 16F877A | 177 | 108 | = / = |
| bench-bitmask | 18F4550 | 161 | 20 | = / = |
| bench-hoist | 18F4550 | 135 | 18 | = / = |

`vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| add-16f877a | 16F877A | 12 | 9 | = / = |
| add-18f4550 | 18F4550 | 19 | 8 | = / = |
| hal-pic16-blink-16f877a | 16F877A | 614 | 47 | = / = |
| hal-pic18-blink-18f4550 | 18F4550 | 396 | 34 | = / = |
| hal-pic16-encoder-full-16f877a | 16F877A | 6909 | 329 | = / = |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 10992 | 755 | = / = |

## Menu-demo clusters (listing words)

| cluster | ours |
|---|---|
| menu_demo_init +7 folded | 1945 |
| main | 638 |
| redraw | 607 |
| gpio4_send | 543 |
| menu_demo_task_heartbeat +1 folded | 469 |
| epic_taskmgr_run +1 folded | 447 |
| EPIC_USART_Init | 433 |
| redraw_status | 398 |
| epic_dispatch_all_irqs +5 folded | 346 |
| epic_dispatch_all_irqs_isr | 329 |
| task_stimulus +1 folded | 257 |
| menu_demo_task_ui | 218 |
| epic_taskmgr_attach_timer0 +2 folded | 208 |

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

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, and rewrites this file. XC8 comparisons render from the private epic-benchmarks repo through this same script.
