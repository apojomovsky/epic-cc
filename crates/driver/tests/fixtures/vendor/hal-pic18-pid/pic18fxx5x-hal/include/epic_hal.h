/*
 * Family-neutral top-level entry point to the PIC18F2455 HAL. Consumers
 * that must build unchanged against any 8-bit PIC family include this
 * instead of `pic18fxx5x.h`; each family ships its own `epic_hal.h`
 * under the same neutral name and the build's include path decides
 * which family's headers are pulled in.
 */

#ifndef EPIC_H
#define EPIC_H

#include "pic18fxx5x.h"       /* standard types, status codes, platform   */
#include "pic18fxx5x_sfr.h"   /* SFR address map + bit definitions       */

/* Core. */
#include "core/pic18_irq.h"
#include "core/pic18fxx5x_wdt_sleep.h"

/* Peripherals. */
#include "peripherals/pic18fxx5x_gpio.h"
#include "peripherals/pic18fxx5x_timer0.h"
#include "peripherals/pic18fxx5x_timer1.h"
#include "peripherals/pic18fxx5x_timer2.h"
#include "peripherals/pic18fxx5x_timer3.h"
#include "peripherals/pic18fxx5x_ccp.h"
#include "peripherals/pic18fxx5x_ssp.h"
#include "peripherals/pic18fxx5x_usart.h"
#include "peripherals/pic18fxx5x_comp.h"
#include "peripherals/pic18fxx5x_eeprom.h"
#include "peripherals/pic18fxx5x_adc.h"
#if PIC18FXX5X_FAMILY_HAS_SPP
#include "peripherals/pic18fxx5x_spp.h"
#endif

#endif /* EPIC_H */
