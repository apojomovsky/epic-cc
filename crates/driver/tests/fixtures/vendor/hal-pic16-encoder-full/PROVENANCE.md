# Provenance

Vendored from `apojomovsky/epic-hal` at commit
`51a048421ca28ebf539251957b895946ade680b8` (origin/master on 2026-10-01): the `epic-encoder` module's full example on `PIC16F87XA`
(target `16F877A`), exactly the combination epic-cc#193 used to
measure and fix the codegen-density gap, refreshed for the size
ladder (epic-cc#836).

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately
(re-run the same file list below against a current epic-hal
checkout) when the drift between this snapshot and real epic-hal
code becomes a concern, e.g. epic-hal adds a new codegen shape
this snapshot doesn't exercise.
`scripts/check-hal-fixture-drift.py --hal-root <epic-hal checkout>`
fails when any file below differs from the pinned commit, and
reports when the pin itself lags that checkout; see its header.

Vendored layout mirrors the upstream tree (`hal/pic14/...`,
`common/...`, `lib/...`), after epic-hal#316 grouped the root
into `common/`, `hal/<arch>/`, `lib/` and `demos/`, and the PIC14
HAL consolidated the per-part drivers into the shared
`hal/pic14/core` slice plus the part table in `hal/pic14/16f87xa`.

Source files (paths relative to this directory):

```
hal/pic14/core/src/peripherals/pic14_gpio.c
hal/pic14/core/src/peripherals/pic14_timer0.c
hal/pic14/core/src/peripherals/pic14_timer2.c
hal/pic14/core/src/peripherals/pic14_ssp.c
hal/pic14/core/src/peripherals/pic14_usart.c
hal/pic14/core/src/core/pic14_irq.c
hal/pic14/core/src/core/pic14_wdt_sleep.c
hal/pic14/16f87xa/src/core/pic16_irq_table.c
hal/pic14/core/src/epiccc/pic14_wdt_sleep_epiccc.c
hal/pic14/core/src/epiccc/pic16_isr_vector.c
hal/pic14/core/src/epiccc/pic16_irq_dispatch_epiccc.c
common/src/core/epic_harness_target.c
lib/tick/src/epic_tick.c
lib/encoder/src/encoder.c
lib/serial/src/epic_serial.c
lib/encoder/examples/example_encoder.c
```

The `epic-encoder` sim variant (epic-cc#931) swaps the target harness for
the sim one and links the serial-tick dispatch instead of the plain
serial dispatch:

```
hal/pic14/core/src/epiccc/pic16_irq_dispatch_serial_tick_epiccc.c
hal/pic14/core/src/epiccc/pic16_irq_dispatch_tiers_inc.h
hal/pic14/16f87xa/src/mdb/pic16_harness_mdb.c
lib/encoder/tests/sim_encoder.c
```

Plus the full `include/` tree of each of `hal/pic14/core`,
`hal/pic14/16f87xa`, `common`, `lib/tick`, `lib/encoder`,
`lib/serial` (headers only, copied wholesale rather than
hand-picked to avoid missing a transitive include).

`config.c` is a hand-written copy of the `EPIC_CONFIG(...)`
translation unit `scripts/epic_build.py` generates for this
module/device in epic-hal, not vendored from a file (it doesn't
exist as a checked-in file there).

Include order used to compile this fixture (device `16F877A`):

```
-Ihal/pic14/16f87xa/include/epiccc -Ihal/pic14/16f87xa/include
-Ihal/pic14/core/include -Icommon/include -Ilib/tick/include
-Ilib/encoder/include -Ilib/serial/include
```

Defines: `PIC16F877A`, `FOSC_HZ=20000000`, `__EPIC_CC__`.
