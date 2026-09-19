# Provenance

Vendored from `apojomovsky/epic-hal` on 2026-09-19 (commit `585eef8`),
the `epic-menu-demo` module's sim variant on `PIC18F4550`: the flagship
dual-toolchain demo behind epic-cc#469, and the largest real PIC18
program epic-cc has been measured against (`LCD + buttons + ADC +
EEPROM + CCP/PWM + taskmgr + tick + serial + the sim harness TU`). It
is the PIC18 counterpart of the `hal-pic16-encoder-full` snapshot,
which does the same job for PIC16.

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately (re-run the
same file list below against a current epic-hal checkout) when the
drift between this snapshot and real epic-hal code becomes a concern,
e.g. epic-hal adds a new codegen shape this snapshot doesn't exercise.

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
epic-tick/src/epic_tick.c
epic-lcd/src/epic_lcd.c
epic-lcd/src/epic_lcd_gpio4.c
epic-serial/src/epic_serial.c
epic-menu-demo/src/menu_demo_core.c
epic-menu-demo/tests/sim_menu_demo.c
```

Plus the full `include/` tree of each of `pic18fxx5x-hal`,
`epic-common`, `epic-taskmgr`, `epic-tick`, `epic-lcd`, `epic-serial`,
`epic-menu-demo` (headers only, copied wholesale rather than
hand-picked to avoid missing a transitive include). One header also
lives beside its source and is copied for the same reason:
`pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_tiers_inc.h`.

`config_18F4550.c` is a copy of the `EPIC_CONFIG(...)` translation unit
`scripts/epic_build.py` generates for this module/device in epic-hal;
it does not exist as a checked-in file there.

Include order used to compile this fixture (device `18F4550`):

```
-Ipic18fxx5x-hal/include/epiccc -Ipic18fxx5x-hal/include
-Iepic-common/include -Iepic-taskmgr/include -Iepic-tick/include
-Iepic-lcd/include -Iepic-serial/include -Iepic-menu-demo/include
```

Defines: `PIC18F4550`, `FOSC_HZ=48000000`, `__EPIC_CC__`.
