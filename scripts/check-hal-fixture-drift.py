#!/usr/bin/env python3
"""Fail when a vendored epic-hal fixture drifted from its pinned commit.

Every size-ladder HAL fixture under
crates/driver/tests/fixtures/vendor is a snapshot: epic-cc's own CI
must stay offline, so nothing here tracks epic-hal live. This script
is the offline-safe guard epic-cc#836 added after the menu-demo row
measured a smaller program than users build (stale __EPIC_CC__
stubs, removed upstream in epic-hal f83922c). It checks two things:

1. Bytes: each vendored file is byte-identical to its upstream path
   at HAL_PIN (read from the hal checkout's git objects, so the
   checkout's worktree state cannot fool it).
2. Pin staleness: HAL_PIN lags the checkout's origin/master, so a
   green byte check on an old pin still nudges a re-vendor.

The generated EPIC_CONFIG translation units (one per demo) are
checked too, by regenerating them with epic-hal's own
scripts/epic_build.py into a temp dir and diffing (the encoder
config.c keeps its historic filename, so only its EPIC_CONFIG line
is compared).

Usage (never runs in cargo tests; the normal suite stays offline):
  python3 scripts/check-hal-fixture-drift.py --hal-root ../epic-hal
  python3 scripts/check-hal-fixture-drift.py --hal-root ../epic-hal --pin HEAD

Exit 0 clean, 1 drifted files, 2 stale pin or unusable checkout.
.github/workflows/hal-fixture-drift.yml runs this weekly against a
fresh epic-hal clone (public repo, no XC8 anywhere near it).
"""

import argparse
import pathlib
import subprocess
import sys
import tempfile

HAL_PIN = "51a048421ca28ebf539251957b895946ade680b8"

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
VENDOR = REPO_ROOT / "crates/driver/tests/fixtures/vendor"

FILES = [
    (
        "hal-pic16-encoder-full/common/include/core/epic_harness.h",
        "common/include/core/epic_harness.h",
    ),
    (
        "hal-pic16-encoder-full/common/include/core/epic_irq.h",
        "common/include/core/epic_irq.h",
    ),
    (
        "hal-pic16-encoder-full/common/include/core/hal_status.h",
        "common/include/core/hal_status.h",
    ),
    (
        "hal-pic16-encoder-full/common/src/core/epic_harness_target.c",
        "common/src/core/epic_harness_target.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/core/hal_irq.h",
        "hal/pic14/16f87xa/include/core/hal_irq.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/core/hal_wdt_sleep.h",
        "hal/pic14/16f87xa/include/core/hal_wdt_sleep.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/core/pic16_irq.h",
        "hal/pic14/16f87xa/include/core/pic16_irq.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/epic_hal.h",
        "hal/pic14/16f87xa/include/epic_hal.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/epiccc/pic16f87xa_platform.h",
        "hal/pic14/16f87xa/include/epiccc/pic16f87xa_platform.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/host/pic16f87xa_platform.h",
        "hal/pic14/16f87xa/include/host/pic16f87xa_platform.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/hal_gpio.h",
        "hal/pic14/16f87xa/include/peripherals/hal_gpio.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/hal_timer0.h",
        "hal/pic14/16f87xa/include/peripherals/hal_timer0.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/hal_timer2.h",
        "hal/pic14/16f87xa/include/peripherals/hal_timer2.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_adc.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_adc.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_ccp.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_ccp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_comp.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_comp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_eeprom.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_eeprom.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_gpio.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_gpio.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_psp.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_psp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_ssp.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_ssp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer0.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer0.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer1.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer1.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer2.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_timer2.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_usart.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_usart.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/peripherals/pic16f87xa_vref.h",
        "hal/pic14/16f87xa/include/peripherals/pic16f87xa_vref.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/pic14_midrange.h",
        "hal/pic14/16f87xa/include/pic14_midrange.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/pic14_midrange_sfr.h",
        "hal/pic14/16f87xa/include/pic14_midrange_sfr.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/pic16f87xa.h",
        "hal/pic14/16f87xa/include/pic16f87xa.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/pic16f87xa_sfr.h",
        "hal/pic14/16f87xa/include/pic16f87xa_sfr.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/pic16f87xa_sim.h",
        "hal/pic14/16f87xa/include/pic16f87xa_sim.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/include/target/pic16f87xa_platform.h",
        "hal/pic14/16f87xa/include/target/pic16f87xa_platform.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/src/core/pic16_irq_table.c",
        "hal/pic14/16f87xa/src/core/pic16_irq_table.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/16f87xa/src/mdb/pic16_harness_mdb.c",
        "hal/pic14/16f87xa/src/mdb/pic16_harness_mdb.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/core/pic14_irq_common.h",
        "hal/pic14/core/include/core/pic14_irq_common.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/core/pic14_wdt_sleep.h",
        "hal/pic14/core/include/core/pic14_wdt_sleep.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_adc.h",
        "hal/pic14/core/include/peripherals/pic14_adc.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_ccp.h",
        "hal/pic14/core/include/peripherals/pic14_ccp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_comp.h",
        "hal/pic14/core/include/peripherals/pic14_comp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_eeprom.h",
        "hal/pic14/core/include/peripherals/pic14_eeprom.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_gpio.h",
        "hal/pic14/core/include/peripherals/pic14_gpio.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_ssp.h",
        "hal/pic14/core/include/peripherals/pic14_ssp.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_timer0.h",
        "hal/pic14/core/include/peripherals/pic14_timer0.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_timer1.h",
        "hal/pic14/core/include/peripherals/pic14_timer1.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_timer2.h",
        "hal/pic14/core/include/peripherals/pic14_timer2.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_usart.h",
        "hal/pic14/core/include/peripherals/pic14_usart.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/include/peripherals/pic14_vref.h",
        "hal/pic14/core/include/peripherals/pic14_vref.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/core/pic14_irq.c",
        "hal/pic14/core/src/core/pic14_irq.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/core/pic14_wdt_sleep.c",
        "hal/pic14/core/src/core/pic14_wdt_sleep.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/epiccc/pic14_wdt_sleep_epiccc.c",
        "hal/pic14/core/src/epiccc/pic14_wdt_sleep_epiccc.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/epiccc/pic16_irq_dispatch_epiccc.c",
        "hal/pic14/core/src/epiccc/pic16_irq_dispatch_epiccc.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/epiccc/pic16_irq_dispatch_serial_tick_epiccc.c",
        "hal/pic14/core/src/epiccc/pic16_irq_dispatch_serial_tick_epiccc.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/epiccc/pic16_irq_dispatch_tiers_inc.h",
        "hal/pic14/core/src/epiccc/pic16_irq_dispatch_tiers_inc.h",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/epiccc/pic16_isr_vector.c",
        "hal/pic14/core/src/epiccc/pic16_isr_vector.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/peripherals/pic14_gpio.c",
        "hal/pic14/core/src/peripherals/pic14_gpio.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/peripherals/pic14_ssp.c",
        "hal/pic14/core/src/peripherals/pic14_ssp.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/peripherals/pic14_timer0.c",
        "hal/pic14/core/src/peripherals/pic14_timer0.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/peripherals/pic14_timer2.c",
        "hal/pic14/core/src/peripherals/pic14_timer2.c",
    ),
    (
        "hal-pic16-encoder-full/hal/pic14/core/src/peripherals/pic14_usart.c",
        "hal/pic14/core/src/peripherals/pic14_usart.c",
    ),
    (
        "hal-pic16-encoder-full/lib/encoder/examples/example_encoder.c",
        "lib/encoder/examples/example_encoder.c",
    ),
    (
        "hal-pic16-encoder-full/lib/encoder/tests/sim_encoder.c",
        "lib/encoder/tests/sim_encoder.c",
    ),
    (
        "hal-pic16-encoder-full/lib/encoder/include/encoder.h",
        "lib/encoder/include/encoder.h",
    ),
    ("hal-pic16-encoder-full/lib/encoder/src/encoder.c", "lib/encoder/src/encoder.c"),
    (
        "hal-pic16-encoder-full/lib/serial/include/epic_serial.h",
        "lib/serial/include/epic_serial.h",
    ),
    (
        "hal-pic16-encoder-full/lib/serial/src/epic_serial.c",
        "lib/serial/src/epic_serial.c",
    ),
    (
        "hal-pic16-encoder-full/lib/tick/include/epic_tick.h",
        "lib/tick/include/epic_tick.h",
    ),
    ("hal-pic16-encoder-full/lib/tick/src/epic_tick.c", "lib/tick/src/epic_tick.c"),
    (
        "hal-pic18-base/epic-adcfilter/include/epic_adcfilter.h",
        "lib/adcfilter/include/epic_adcfilter.h",
    ),
    (
        "hal-pic18-base/epic-adcfilter/src/epic_adcfilter.c",
        "lib/adcfilter/src/epic_adcfilter.c",
    ),
    ("hal-pic18-base/epic-bus/include/epic_bus.h", "lib/bus/include/epic_bus.h"),
    ("hal-pic18-base/epic-bus/src/epic_bus.c", "lib/bus/src/epic_bus.c"),
    (
        "hal-pic18-base/epic-common/include/core/epic_harness.h",
        "common/include/core/epic_harness.h",
    ),
    (
        "hal-pic18-base/epic-common/include/core/epic_irq.h",
        "common/include/core/epic_irq.h",
    ),
    (
        "hal-pic18-base/epic-common/include/core/hal_status.h",
        "common/include/core/hal_status.h",
    ),
    ("hal-pic18-base/epic-math/include/epic_math.h", "lib/math/include/epic_math.h"),
    (
        "hal-pic18-base/epic-math/src/common/epic_math_numeric.c",
        "lib/math/src/common/epic_math_numeric.c",
    ),
    (
        "hal-pic18-base/epic-math/src/common/epic_math_rand.c",
        "lib/math/src/common/epic_math_rand.c",
    ),
    (
        "hal-pic18-base/epic-math/src/common/epic_math_sqrt.c",
        "lib/math/src/common/epic_math_sqrt.c",
    ),
    (
        "hal-pic18-base/epic-math/src/pic18/epic_math_addsub.c",
        "lib/math/src/pic18/epic_math_addsub.c",
    ),
    (
        "hal-pic18-base/epic-math/src/pic18/epic_math_bcd.c",
        "lib/math/src/pic18/epic_math_bcd.c",
    ),
    (
        "hal-pic18-base/epic-math/src/pic18/epic_math_div.c",
        "lib/math/src/pic18/epic_math_div.c",
    ),
    (
        "hal-pic18-base/epic-math/src/pic18/epic_math_mul.c",
        "lib/math/src/pic18/epic_math_mul.c",
    ),
    (
        "hal-pic18-base/epic-math/tests/epic_math_test.h",
        "lib/math/tests/epic_math_test.h",
    ),
    (
        "hal-pic18-base/epic-math/tests/golden_vectors.h",
        "lib/math/tests/golden_vectors.h",
    ),
    (
        "hal-pic18-base/epic-mcp23x17/include/epic_mcp23x17.h",
        "lib/mcp23x17/include/epic_mcp23x17.h",
    ),
    (
        "hal-pic18-base/epic-mcp23x17/src/epic_mcp23x17.c",
        "lib/mcp23x17/src/epic_mcp23x17.c",
    ),
    (
        "hal-pic18-base/epic-modbus/include/epic_modbus.h",
        "lib/modbus/include/epic_modbus.h",
    ),
    ("hal-pic18-base/epic-modbus/src/epic_modbus.c", "lib/modbus/src/epic_modbus.c"),
    ("hal-pic18-base/epic-pid/include/pid.h", "lib/pid/include/pid.h"),
    ("hal-pic18-base/epic-pid/src/pid.c", "lib/pid/src/pid.c"),
    (
        "hal-pic18-base/epic-serial/include/epic_serial.h",
        "lib/serial/include/epic_serial.h",
    ),
    ("hal-pic18-base/epic-serial/src/epic_serial.c", "lib/serial/src/epic_serial.c"),
    (
        "hal-pic18-base/epic-taskmgr/include/epic_taskmgr.h",
        "lib/taskmgr/include/epic_taskmgr.h",
    ),
    (
        "hal-pic18-base/epic-taskmgr/src/epic_taskmgr.c",
        "lib/taskmgr/src/epic_taskmgr.c",
    ),
    ("hal-pic18-base/epic-tick/include/epic_tick.h", "lib/tick/include/epic_tick.h"),
    ("hal-pic18-base/epic-tick/src/epic_tick.c", "lib/tick/src/epic_tick.c"),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/core/hal_irq.h",
        "hal/pic18/18fxx5x/include/core/hal_irq.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/core/hal_wdt_sleep.h",
        "hal/pic18/18fxx5x/include/core/hal_wdt_sleep.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/core/pic18_irq.h",
        "hal/pic18/18fxx5x/include/core/pic18_irq.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/core/pic18fxx5x_wdt_sleep.h",
        "hal/pic18/18fxx5x/include/core/pic18fxx5x_wdt_sleep.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/epic_hal.h",
        "hal/pic18/18fxx5x/include/epic_hal.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/epiccc/pic18_platform.h",
        "hal/pic18/18fxx5x/include/epiccc/pic18_platform.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/host/pic18_platform.h",
        "hal/pic18/18fxx5x/include/host/pic18_platform.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/hal_gpio.h",
        "hal/pic18/18fxx5x/include/peripherals/hal_gpio.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/hal_timer0.h",
        "hal/pic18/18fxx5x/include/peripherals/hal_timer0.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/hal_timer2.h",
        "hal/pic18/18fxx5x/include/peripherals/hal_timer2.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_adc.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_adc.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_ccp.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_ccp.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_comp.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_comp.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_eeprom.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_eeprom.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_gpio.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_gpio.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_spp.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_spp.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_ssp.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_ssp.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_timer0.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_timer0.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_timer1.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_timer1.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_timer2.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_timer2.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_timer3.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_timer3.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/peripherals/pic18fxx5x_usart.h",
        "hal/pic18/18fxx5x/include/peripherals/pic18fxx5x_usart.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/pic18fxx5x.h",
        "hal/pic18/18fxx5x/include/pic18fxx5x.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/pic18fxx5x_sfr.h",
        "hal/pic18/18fxx5x/include/pic18fxx5x_sfr.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/pic18fxx5x_sim.h",
        "hal/pic18/18fxx5x/include/pic18fxx5x_sim.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/include/target/pic18_platform.h",
        "hal/pic18/18fxx5x/include/target/pic18_platform.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/core/pic18_irq.c",
        "hal/pic18/18fxx5x/src/core/pic18_irq.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
        "hal/pic18/18fxx5x/src/core/pic18fxx5x_wdt_sleep.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
        "hal/pic18/18fxx5x/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_tiers_inc.h",
        "hal/pic18/18fxx5x/src/epiccc/pic18_irq_dispatch_tiers_inc.h",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
        "hal/pic18/18fxx5x/src/epiccc/pic18_isr_vector.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
        "hal/pic18/18fxx5x/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_adc.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_ccp.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_eeprom.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_gpio.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ssp.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_ssp.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_timer0.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_timer2.c",
    ),
    (
        "hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
        "hal/pic18/18fxx5x/src/peripherals/pic18fxx5x_usart.c",
    ),
    (
        "hal-pic18-bridge-demo/epic-bridge-demo/include/bridge_demo_core.h",
        "demos/bridge/include/bridge_demo_core.h",
    ),
    (
        "hal-pic18-bridge-demo/epic-bridge-demo/src/bridge_demo_core.c",
        "demos/bridge/src/bridge_demo_core.c",
    ),
    (
        "hal-pic18-bridge-demo/epic-bridge-demo/tests/sim_bridge_demo.c",
        "demos/bridge/tests/sim_bridge_demo.c",
    ),
    (
        "hal-pic18-bridge-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
        "hal/pic18/18fxx5x/src/mdb/pic18_harness_mdb.c",
    ),
    (
        "hal-pic18-control-demo/epic-control-demo/include/control_demo_core.h",
        "demos/control/include/control_demo_core.h",
    ),
    (
        "hal-pic18-control-demo/epic-control-demo/src/control_demo_core.c",
        "demos/control/src/control_demo_core.c",
    ),
    (
        "hal-pic18-control-demo/epic-control-demo/tests/sim_control_demo.c",
        "demos/control/tests/sim_control_demo.c",
    ),
    (
        "hal-pic18-control-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
        "hal/pic18/18fxx5x/src/mdb/pic18_harness_mdb.c",
    ),
    ("hal-pic18-menu-demo/epic-lcd/include/epic_lcd.h", "lib/lcd/include/epic_lcd.h"),
    (
        "hal-pic18-menu-demo/epic-lcd/include/epic_lcd_transport.h",
        "lib/lcd/include/epic_lcd_transport.h",
    ),
    ("hal-pic18-menu-demo/epic-lcd/src/epic_lcd.c", "lib/lcd/src/epic_lcd.c"),
    (
        "hal-pic18-menu-demo/epic-lcd/src/epic_lcd_gpio4.c",
        "lib/lcd/src/epic_lcd_gpio4.c",
    ),
    (
        "hal-pic18-menu-demo/epic-menu-demo/include/menu_demo_core.h",
        "demos/menu/include/menu_demo_core.h",
    ),
    (
        "hal-pic18-menu-demo/epic-menu-demo/src/menu_demo_core.c",
        "demos/menu/src/menu_demo_core.c",
    ),
    (
        "hal-pic18-menu-demo/epic-menu-demo/tests/sim_menu_demo.c",
        "demos/menu/tests/sim_menu_demo.c",
    ),
    (
        "hal-pic18-menu-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
        "hal/pic18/18fxx5x/src/mdb/pic18_harness_mdb.c",
    ),
    ("hal-pic18-pid/epic-pid/tests/sim_pid.c", "lib/pid/tests/sim_pid.c"),
    (
        "hal-pic18-pid/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
        "hal/pic18/18fxx5x/src/mdb/pic18_harness_mdb.c",
    ),
]

SKIP_NAMES = {"PROVENANCE.md"}

CONFIGS = [
    (
        "epic-menu-demo",
        "18F4550",
        "sim",
        "hal-pic18-menu-demo/config_18F4550.c",
        "config_18F4550.c",
    ),
    (
        "epic-control-demo",
        "18F4550",
        "sim",
        "hal-pic18-control-demo/config_18F4550.c",
        "config_18F4550.c",
    ),
    (
        "epic-pid",
        "18F4550",
        "sim",
        "hal-pic18-pid/config_18F4550.c",
        "config_18F4550.c",
    ),
    (
        "epic-bridge-demo",
        "18F4550",
        "sim",
        "hal-pic18-bridge-demo/config_18F4550.c",
        "config_18F4550.c",
    ),
]

ENCODER_CONFIG = (
    "epic-encoder",
    "16F877A",
    "target",
    "hal-pic16-encoder-full/config.c",
    "config_16F877A.c",
)


def run(*args, cwd=None):
    return subprocess.run(
        args,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def git_show(hal_root, path):
    p = run("git", "-C", str(hal_root), "show", f"{HAL_PIN}:{path}")
    if p.returncode != 0:
        return None
    return p.stdout.encode()


def check_files(hal_root, pin):
    failures = []
    for vendored_rel, upstream_rel in FILES:
        local = VENDOR / vendored_rel
        if not local.is_file():
            failures.append(f"missing vendored file: {vendored_rel}")
            continue
        p = run("git", "-C", str(hal_root), "show", f"{pin}:{upstream_rel}")
        if p.returncode != 0:
            failures.append(f"missing upstream blob: {upstream_rel}")
            continue
        if local.read_bytes() != p.stdout.encode():
            failures.append(f"drifted: {vendored_rel} != {upstream_rel}@{pin[:7]}")
    mapped = {v for v, _ in FILES}
    for local in sorted(VENDOR.rglob("*")):
        if not local.is_file() or local.name in SKIP_NAMES:
            continue
        rel = str(local.relative_to(VENDOR))
        if (
            rel not in mapped
            and not rel.endswith("config.c")
            and not rel.endswith("config_18F4550.c")
        ):
            failures.append(f"unmapped vendored file: {rel} (extend FILES)")
    return failures


def generated_config(hal_root, module, mcu, variant):
    with tempfile.TemporaryDirectory() as tmp:
        p = run(
            sys.executable,
            "scripts/epic_build.py",
            "build",
            "--module",
            module,
            "--mcu",
            mcu,
            "--variant",
            variant,
            "--toolchain",
            "epic-cc",
            "--epic-cc",
            "true",
            "--build-dir",
            tmp,
            cwd=str(hal_root),
        )
        if p.returncode != 0:
            return None, p.stderr.strip().splitlines()[-1:]
        matches = list(pathlib.Path(tmp).rglob("config_*.c"))
        if len(matches) != 1:
            return None, [f"expected one config, found {len(matches)}"]
        return matches[0].read_bytes(), []


def config_line(blob):
    for line in blob.decode().splitlines():
        if line.startswith("EPIC_CONFIG("):
            return line
    return ""


def check_configs(hal_root):
    failures = []
    for module, mcu, variant, vendored_rel, name in CONFIGS:
        blob, err = generated_config(hal_root, module, mcu, variant)
        if blob is None:
            failures.append(f"config regen failed for {module}: {err}")
            continue
        if (VENDOR / vendored_rel).read_bytes() != blob:
            failures.append(f"drifted config: {vendored_rel} (regen {module})")
    module, mcu, variant, vendored_rel, _ = ENCODER_CONFIG
    blob, err = generated_config(hal_root, module, mcu, variant)
    if blob is None:
        failures.append(f"config regen failed for {module}: {err}")
    elif config_line((VENDOR / vendored_rel).read_bytes()) != config_line(blob):
        failures.append(f"drifted config: {vendored_rel} (regen {module})")
    return failures


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--hal-root", required=True)
    ap.add_argument("--pin", default=HAL_PIN)
    args = ap.parse_args()
    hal_root = pathlib.Path(args.hal_root)
    if not (hal_root / "scripts/epic_build.py").is_file():
        print(f"not an epic-hal checkout: {hal_root}")
        return 2
    if (
        run(
            "git", "-C", str(hal_root), "cat-file", "-e", f"{args.pin}^{{commit}}"
        ).returncode
        != 0
    ):
        print(f"pin not found in checkout: {args.pin}")
        return 2
    failures = check_files(hal_root, args.pin)
    failures += check_configs(hal_root)
    for f in failures:
        print(f)
    if failures:
        print(f"{len(failures)} drifted file(s) at pin {args.pin[:7]}")
        return 1
    tip = run("git", "-C", str(hal_root), "rev-parse", "origin/master")
    tip_sha = tip.stdout.strip() if tip.returncode == 0 else ""
    if tip_sha and tip_sha != args.pin:
        print(f"stale pin: {args.pin[:7]} lags origin/master {tip_sha[:7]}")
        return 2
    print(f"fixtures match epic-hal@{args.pin[:7]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
