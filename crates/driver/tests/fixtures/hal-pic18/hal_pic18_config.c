/* EPIC_CONFIG for the PIC18F4550 smoke: the fuses the epic-hal manifest
 * uses for epic-tick on 18F4550, in epic-cc's spelling. 20 MHz crystal,
 * PLL enabled (20 MHz / PLLDIV5 = 4 MHz PLL input, 96 MHz / CPUDIV div1 =
 * 48 MHz system clock), USB divisor enabled, watchdog off, low-voltage
 * programming off. `xtal_hz` lets the driver derive EPIC_FOSC_HZ = 48 MHz.
 */

#include <epic-cc.h>

EPIC_CONFIG("osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, wdt=off, lvp=off, xtal_hz=20000000");
