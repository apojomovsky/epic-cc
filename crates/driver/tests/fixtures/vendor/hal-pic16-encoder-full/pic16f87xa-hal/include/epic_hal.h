/* Family-neutral entry point to the PIC16F87XA HAL. Consumers that must
 * build unchanged across 8-bit PIC families include this name instead
 * of pic16f87xa.h; the build's include path selects the family's copy. */

#ifndef EPIC_H
#define EPIC_H

#include "pic16f87xa.h"       /* standard types, status codes, platform   */
#include "pic16f87xa_sfr.h"   /* SFR address map + bit definitions       */

/* Core. */
#include "core/pic16_irq.h"
#include "core/pic16f87xa_wdt_sleep.h"

/* Peripherals. */
#include "peripherals/pic16f87xa_gpio.h"
#include "peripherals/pic16f87xa_timer0.h"
#include "peripherals/pic16f87xa_timer1.h"
#include "peripherals/pic16f87xa_timer2.h"
#include "peripherals/pic16f87xa_ccp.h"
#include "peripherals/pic16f87xa_usart.h"
#include "peripherals/pic16f87xa_ssp.h"
#include "peripherals/pic16f87xa_adc.h"
#include "peripherals/pic16f87xa_comp.h"
#include "peripherals/pic16f87xa_vref.h"
#include "peripherals/pic16f87xa_eeprom.h"
#if PIC16F87XA_FAMILY_HAS_PSP
#include "peripherals/pic16f87xa_psp.h"
#endif

#endif /* EPIC_H */
