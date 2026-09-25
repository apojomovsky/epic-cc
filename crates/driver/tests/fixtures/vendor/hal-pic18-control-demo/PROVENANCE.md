# Provenance

Vendored from `apojomovsky/epic-hal` on 2026-09-25 (commit `879834c`),
the `epic-control-demo` module's sim variant on `18F4550` (epic-cc
toolchain file set), for the size ladder (epic-cc#677).

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately (resolve `sources_for` with the epic-cc toolchain and regenerate the config via `scripts/epic_build.py`, as in epic-cc#677) when the drift between this snapshot and real epic-hal code becomes a concern, e.g. epic-hal adds a new codegen shape this snapshot does not exercise.

Source files (paths relative to the epic-hal repo root):

```
pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c
pic18fxx5x-hal/src/core/pic18_irq.c
pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c
pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c
pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c
pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c
pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c
epic-taskmgr/src/epic_taskmgr.c
epic-math/src/common/epic_math_numeric.c
epic-math/src/common/epic_math_rand.c
epic-math/src/common/epic_math_sqrt.c
epic-math/src/pic18/epic_math_addsub.c
epic-math/src/pic18/epic_math_bcd.c
epic-math/src/pic18/epic_math_div.c
epic-math/src/pic18/epic_math_mul.c
epic-pid/src/pid.c
epic-adcfilter/src/epic_adcfilter.c
epic-serial/src/epic_serial.c
epic-control-demo/src/control_demo_core.c
epic-control-demo/tests/sim_control_demo.c
```

Plus the full `include/` tree of each of `pic18fxx5x-hal`,
`epic-common`, `epic-taskmgr`, `epic-math`, `epic-pid`,
`epic-adcfilter`, `epic-serial`, `epic-control-demo` (headers only,
copied wholesale rather than hand-picked to avoid missing a
transitive include). One header also lives beside its source and is
copied for the same reason:
`pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_tiers_inc.h`.

`config_18F4550.c` is a copy of the `EPIC_CONFIG(...)` translation unit
`scripts/epic_build.py` generates for this module/device in epic-hal
(`--toolchain epic-cc` variant, byte-identical to the menu-demo one);
it does not exist as a checked-in file there.

Include order used to compile this fixture (device `18F4550`):

```
-Ipic18fxx5x-hal/include/epiccc -Ipic18fxx5x-hal/include
-Iepic-common/include -Iepic-taskmgr/include -Iepic-math/include
-Iepic-pid/include -Iepic-adcfilter/include -Iepic-serial/include
-Iepic-control-demo/include
```

Defines: `PIC18F4550`, `FOSC_HZ=48000000`, `__EPIC_CC__`.
