# Size ladder report

Generated from the tree, not by hand. Every epic-cc number below was measured by `size_regression_e2e.rs` on this commit.

- Date (UTC): 2026-10-08
- Commit: 6f4a58e
- Baseline: crates/driver/tests/fixtures/size_baseline.toml (checked in)
- Menu-demo listing: 9164 words (assembler input; far-branch expansion and PCL padding account for the residual to the driver total)

## Micro benches (flash / RAM)

| bench | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| bench-shift | 18F4550 | 88 | 22 | = / = |
| bench-wide-const | 18F4550 | 23 | 6 | = / = |
| bench-zero-init | 18F4550 | 25 | 12 | = / = |
| bench-struct-copy | 18F4550 | 35 | 45 | = / = |
| bench-counted-copy | 18F4550 | 76 | 25 | = / = |
| bench-switch | 18F4550 | 66 | 5 | = / = |
| bench-bool | 18F4550 | 35 | 4 | = / = |
| bench-const-sub | 18F4550 | 25 | 5 | = / = |
| bench-dead-store | 18F4550 | 16 | 3 | = / = |
| bench-w-roundtrip | 18F4550 | 18 | 4 | = / = |
| bench-bank | 18F4550 | 23 | 10 | = / = |
| bench-handle-init | 18F4550 | 21 | 4 | = / = |
| bench-switch-calls | 18F4550 | 49 | 6 | = / = |
| bench-u32-loop | 18F4550 | 48 | 19 | = / = |
| bench-u16-dec | 18F4550 | 67 | 21 | = / = |
| bench-struct-scan | 18F4550 | 103 | 95 | = / = |
| bench-struct-scan | 16F877A | 166 | 104 | = / = |
| bench-plusw | 18F4550 | 40 | 23 | = / = |
| bench-branch-computed | 18F4550 | 38 | 5 | = / = |
| bench-bitmask | 18F4550 | 52 | 14 | = / = |
| bench-hoist | 18F4550 | 116 | 14 | = / = |

`vs baseline` is flash / RAM delta against the checked-in pins.

## Whole programs (flash / RAM)

| program | device | epic-cc flash | epic-cc RAM | vs baseline |
|---|---|---|---|---|
| add-16f877a | 16F877A | 10 | 3 | = / = |
| add-18f4550 | 18F4550 | 16 | 2 | = / = |
| hal-pic16-blink-16f877a | 16F877A | 462 | 44 | = / = |
| hal-pic18-blink-18f4550 | 18F4550 | 313 | 30 | = / = |
| hal-pic14e-blink-16f1937 | 16F1937 | 100 | 6 | = / = |
| hal-pic16-encoder-full-16f877a | 16F877A | 5372 | 326 | = / = |
| hal-pic18-menu-demo-18f4550 | 18F4550 | 9177 | 781 | = / = |
| hal-pic18-control-demo-18f4550 | 18F4550 | 9463 | 963 | = / = |
| hal-pic18-pid-18f4550 | 18F4550 | 3202 | 500 | = / = |
| hal-pic18-bridge-demo-18f4550 | 18F4550 | 13291 | 1312 | = / = |

## Menu-demo clusters (listing words)

| cluster | ours |
|---|---|
| menu_demo_init +7 folded | 1707 |
| redraw | 464 |
| main | 367 |
| EPIC_USART_Init | 327 |
| epic_taskmgr_run +1 folded | 322 |
| redraw_status | 308 |
| epic_dispatch_all_irqs_isr | 267 |
| menu_demo_task_heartbeat +1 folded | 264 |
| epic_dispatch_all_irqs +5 folded | 235 |
| menu_demo_task_ui | 208 |
| __epic_config | 196 |
| EPIC_IRQ_GetFlag_isr | 185 |
| epic_taskmgr_attach_timer0 +2 folded | 183 |

Top profiler categories on the same listing:

- `data-move`: 1408 (15.4%)
- `shared-code`: 1321 (14.4%)
- `struct-copy-movff`: 1184 (12.9%)
- `branch`: 763 (8.3%)
- `scalar-alu`: 587 (6.4%)
- `cond-branch`: 555 (6.1%)
- `literal-load`: 485 (5.3%)
- `slot-copy`: 404 (4.4%)

## Regenerating

```
make size-report
```

The target recompiles every ladder case through the real driver, rebuilds the menu-demo listing for the cluster table, and rewrites this file. XC8 comparisons render from the private epic-benchmarks repo through this same script.
