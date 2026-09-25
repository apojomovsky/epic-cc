# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit.

- Date (UTC): 2026-09-25
- Commit: c7a493e
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- Menu-demo listing: 9585 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| bench-shift | 18F4550 | 88 | 26 | = / = |
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
| bench-u16-dec | 18F4550 | 116 | 33 | = / = |
| bench-struct-scan | 18F4550 | 119 | 99 | = / = |
| bench-struct-scan | 16F877A | 177 | 108 | = / = |
| bench-bitmask | 18F4550 | 157 | 20 | = / = |
| bench-hoist | 18F4550 | 120 | 18 | = / = |

`vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| add-16f877a | 16F877A | 12 | 9 | = / = |
| add-18f4550 | 18F4550 | 19 | 8 | = / = |
| hal-pic16-blink-16f877a | 16F877A | 614 | 47 | = / = |
| hal-pic18-blink-18f4550 | 18F4550 | 359 | 34 | = / = |
| hal-pic16-encoder-full-16f877a | 16F877A | 6909 | 329 | = / = |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 9677 | 755 | = / = |

## Menu-demo clusters (listing words)

| cluster | ours |
|---|---|
| menu_demo_init +7 folded | 1561 |
| redraw | 522 |
| main | 410 |
| redraw_status | 366 |
| epic_taskmgr_run +1 folded | 343 |
| EPIC_USART_Init | 340 |
| epic_dispatch_all_irqs_isr | 329 |
| epic_dispatch_all_irqs +5 folded | 297 |
| menu_demo_task_heartbeat +1 folded | 231 |
| gpio4_send | 224 |
| menu_demo_task_ui | 212 |
| __epic_config | 196 |
| epic_serial_put_u16 | 194 |

Top profiler categories on the same listing:

- `other`: 4799 (50.1%)
- `shared-code`: 1427 (14.9%)
- `struct-copy-movff`: 1342 (14.0%)
- `wide-literal-arith`: 465 (4.9%)
- `const-data`: 356 (3.7%)
- `wide-const-materialization`: 262 (2.7%)
- `runtime-routine`: 236 (2.5%)
- `switch-jump-table`: 212 (2.2%)

## Regenerating

```
make size-report
```

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, and rewrites this file. XC8 comparisons render from the private epic-benchmarks repo through this same script.
