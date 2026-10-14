# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit.

- Date (UTC): 2026-10-14
- Commit: 62cac50
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- Menu-demo listing: 7562 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| bench-shift | 18F4550 | 88 | 22 | = / = |
| bench-wide-const | 18F4550 | 23 | 6 | = / = |
| bench-zero-init | 18F4550 | 25 | 12 | = / = |
| bench-struct-copy | 18F4550 | 33 | 45 | = / = |
| bench-counted-copy | 18F4550 | 64 | 23 | -8 / -2 |
| bench-switch | 18F4550 | 58 | 3 | = / -2 |
| bench-bool | 18F4550 | 35 | 4 | = / = |
| bench-const-sub | 18F4550 | 25 | 5 | = / = |
| bench-dead-store | 18F4550 | 16 | 3 | = / = |
| bench-w-roundtrip | 18F4550 | 16 | 3 | = / = |
| bench-bank | 18F4550 | 23 | 10 | = / = |
| bench-handle-init | 18F4550 | 21 | 4 | = / = |
| bench-switch-calls | 18F4550 | 42 | 6 | -2 / = |
| bench-u32-loop | 18F4550 | 47 | 13 | = / -6 |
| bench-u16-dec | 18F4550 | 67 | 21 | = / = |
| bench-u16-dec-rj5 | 18F4550 | 357 | 51 | -27 / -6 |
| bench-struct-scan | 18F4550 | 87 | 95 | -4 / = |
| bench-struct-scan | 16F877A | 162 | 104 | = / = |
| bench-plusw | 18F4550 | 36 | 23 | = / = |
| bench-indf-rmw | 18F4550 | 40 | 71 | -1 / = |
| bench-branch-computed | 18F4550 | 35 | 5 | = / = |
| bench-bitmask | 18F4550 | 52 | 14 | = / = |
| bench-hoist | 18F4550 | 116 | 14 | = / = |

`vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| add-16f877a | 16F877A | 10 | 3 | = / = |
| add-18f4550 | 18F4550 | 16 | 2 | = / = |
| add-p16f1939 | 16F1939 | 11 | 3 | = / = |
| hal-pic16-blink-16f877a | 16F877A | 363 | 40 | -78 / = |
| hal-pic18-blink-18f4550 | 18F4550 | 271 | 20 | -48 / -2 |
| hal-pic14e-blink-16f1937 | 16F1937 | 61 | 6 | -39 / = |
| hal-pic16-encoder-full-16f877a | 16F877A | 4006 | 277 | -1154 / -24 |
| hal-pic16-encoder-sim-16f877a | 16F877A | 4605 | 231 | -1980 / -31 |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 7617 | 655 | -1011 / -84 |
| hal-pic18-control-demo-18f4550 | 18F4550 | 8595 | 828 | -462 / -133 |
| hal-pic18-pid-18f4550 | 18F4550 | 2872 | 379 | -260 / -117 |
| hal-pic18-bridge-demo-18f4550 | 18F4550 | 11560 | 1067 | -1348 / -211 |

## Menu-demo clusters (listing words)

| cluster | ours |
|---|---|
| menu_demo_init +7 folded | 1242 |
| EPIC_USART_Init | 324 |
| epic_taskmgr_run +1 folded | 324 |
| main | 314 |
| redraw | 268 |
| epic_dispatch_all_irqs_isr | 251 |
| epic_dispatch_all_irqs +5 folded | 224 |
| menu_demo_task_heartbeat +1 folded | 206 |
| menu_demo_task_ui | 205 |
| epic_taskmgr_attach_timer0 +2 folded | 187 |
| EPIC_IRQ_GetFlag_isr | 185 |
| gpio4_send | 166 |
| EPIC_IRQ_ClearFlag_isr | 145 |

Top profiler categories on the same listing:

- `data-move`: 1180 (15.6%)
- `shared-code`: 1149 (15.2%)
- `struct-copy-movff`: 850 (11.2%)
- `branch`: 702 (9.3%)
- `scalar-alu`: 500 (6.6%)
- `literal-load`: 469 (6.2%)
- `cond-branch`: 438 (5.8%)
- `slot-copy`: 396 (5.2%)

## Regenerating

```
make size-report
```

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, and rewrites this file. XC8 comparisons render from the private epic-benchmarks repo through this same script.
