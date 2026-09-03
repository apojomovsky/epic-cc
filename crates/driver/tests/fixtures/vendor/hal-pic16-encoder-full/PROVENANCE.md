# Provenance

Vendored from `apojomovsky/epic-hal` on 2026-09-03, the `epic-encoder`
module's full example on `PIC16F87XA` (target `16F877A`), exactly the
combination epic-cc#193 used to measure and fix the codegen-density gap:
`gpio+timer0+timer2+ssp+usart+irq+wdt+dispatch+tick+encoder+serial+the
example TU`.

This is a **snapshot**, not a live sync: epic-cc's own CI must not depend
on epic-hal's current state, so nothing here updates automatically when
epic-hal changes. Re-vendor deliberately (re-run the same file list
below against a current epic-hal checkout) when the drift between this
snapshot and real epic-hal code becomes a concern, e.g. epic-hal adds a
new codegen shape this snapshot doesn't exercise.

**Re-vendored 2026-09-03** (`epic-serial/include/epic_serial.h`,
`epic-serial/src/epic_serial.c` only): apojomovsky/epic-hal#123/#124,
the `printf`-literal staging buffer consolidation (epic-cc#206).

Source files (paths relative to the epic-hal repo root):

```
pic16f87xa-hal/src/peripherals/pic16f87xa_gpio.c
pic16f87xa-hal/src/peripherals/pic16f87xa_timer0.c
pic16f87xa-hal/src/peripherals/pic16f87xa_timer2.c
pic16f87xa-hal/src/peripherals/pic16f87xa_ssp.c
pic16f87xa-hal/src/peripherals/pic16f87xa_usart.c
pic16f87xa-hal/src/core/pic16_irq.c
pic16f87xa-hal/src/core/pic16f87xa_wdt_sleep.c
pic16f87xa-hal/src/epiccc/pic16f87xa_wdt_sleep_epiccc.c
pic16f87xa-hal/src/epiccc/pic16_isr_vector.c
pic16f87xa-hal/src/epiccc/pic16_irq_dispatch_epiccc.c
epic-common/src/core/epic_harness_target.c
epic-tick/src/epic_tick.c
epic-encoder/src/encoder.c
epic-serial/src/epic_serial.c
epic-encoder/examples/example_encoder.c
```

Plus the full `include/` tree of each of `pic16f87xa-hal`, `epic-common`,
`epic-tick`, `epic-encoder`, `epic-serial` (headers only, copied
wholesale rather than hand-picked to avoid missing a transitive include).

`config.c` is a hand-written copy of the `EPIC_CONFIG(...)` translation
unit `scripts/epic_build.py` generates for this module/device in
epic-hal, not vendored from a file (it doesn't exist as a checked-in
file there).

Include order used to compile this fixture (device `16F877A`):

```
-Ipic16f87xa-hal/include/epiccc -Ipic16f87xa-hal/include
-Iepic-common/include -Iepic-tick/include
-Iepic-encoder/include -Iepic-serial/include
```
