# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit.

- Date (UTC): 2026-09-30
- Commit: 383bae0
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- Menu-demo listing: 9616 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| bench-shift | 18F4550 | 88 | 26 | = / = |
| bench-wide-const | 18F4550 | 23 | 10 | = / = |
| bench-zero-init | 18F4550 | 25 | 16 | = / = |
| bench-struct-copy | 18F4550 | 37 | 51 | = / = |
| bench-switch | 18F4550 | 66 | 9 | = / = |
| bench-bool | 18F4550 | 37 | 9 | = / = |
| bench-dead-store | 18F4550 | 19 | 8 | = / = |
| bench-w-roundtrip | 18F4550 | 20 | 10 | = / = |
| bench-bank | 18F4550 | 33 | 15 | = / = |
| bench-handle-init | 18F4550 | 23 | 9 | = / = |
| bench-switch-calls | 18F4550 | 49 | 10 | = / = |
| bench-u32-loop | 18F4550 | 68 | 23 | = / = |
| bench-u16-dec | 18F4550 | 109 | 33 | = / = |
| bench-struct-scan | 18F4550 | 109 | 99 | = / = |
| bench-struct-scan | 16F877A | 168 | 108 | = / = |
| bench-plusw | 18F4550 | 42 | 27 | = / = |
| bench-branch-computed | 18F4550 | 42 | 9 | = / = |
| bench-bitmask | 18F4550 | 64 | 20 | = / = |
| bench-hoist | 18F4550 | 116 | 18 | = / = |

`vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| add-16f877a | 16F877A | 12 | 9 | = / = |
| add-18f4550 | 18F4550 | 18 | 8 | = / = |
| hal-pic16-blink-16f877a | 16F877A | 471 | 47 | = / = |
| hal-pic18-blink-18f4550 | 18F4550 | 331 | 34 | = / = |
| hal-pic16-encoder-full-16f877a | 16F877A | 5460 | 326 | = / = |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 9629 | 779 | = / = |
| hal-pic18-control-demo-18f4550 | 18F4550 | 9962 | 977 | = / = |
| hal-pic18-pid-18f4550 | 18F4550 | 3309 | 500 | = / = |
| hal-pic18-bridge-demo-18f4550 | 18F4550 | 14026 | 1312 | = / = |

## Menu-demo clusters (listing words)

| cluster | ours |
|---|---|
| menu_demo_init +7 folded | 1705 |
| redraw | 523 |
| main | 385 |
| redraw_status | 367 |
| EPIC_USART_Init | 329 |
| epic_taskmgr_run +1 folded | 328 |
| epic_dispatch_all_irqs_isr | 287 |
| epic_dispatch_all_irqs +5 folded | 263 |
| gpio4_send | 263 |
| menu_demo_task_heartbeat +1 folded | 226 |
| menu_demo_task_ui | 211 |
| __epic_config | 196 |
| EPIC_IRQ_GetFlag_isr | 187 |

Top profiler categories on the same listing:

- `shared-code`: 1537 (16.0%)
- `data-move`: 1323 (13.8%)
- `struct-copy-movff`: 1284 (13.4%)
- `branch`: 769 (8.0%)
- `scalar-alu`: 537 (5.6%)
- `cond-branch`: 530 (5.5%)
- `literal-load`: 507 (5.3%)
- `slot-copy`: 456 (4.7%)

## Regenerating

```
make size-report
```

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, and rewrites this file. XC8 comparisons render from the private epic-benchmarks repo through this same script.
