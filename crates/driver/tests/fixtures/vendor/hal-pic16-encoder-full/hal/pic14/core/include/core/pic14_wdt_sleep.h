/* Shared PIC14 mid-range CPU helpers: Watchdog Timer, Brown-out
 * Reset, Sleep. Sources: DS39582B §14 (87XA), DS40001291H §14 (88X).
 * Config bits are left to the user's MPLAB X/XC8 setup;
 * EPIC_Sleep_Enter is a no-op on host. The 88X adds WDTCON software
 * control (SWDTEN/prescaler); parts without WDTCON (87XA, 628A) run
 * the same file with an empty software block. Parts without PCON at
 * all (83/84/84A, no BOR either) compile out the BOR/POR helpers. */

#ifndef PIC14_WDT_SLEEP_H
#define PIC14_WDT_SLEEP_H

#include "pic14_midrange.h"
#include "pic14_midrange_sfr.h"

/**
 * @brief  Refresh the Watchdog Timer (`clrwdt`, no-op on host). Must be
 *         called more often than the WDT period (typ. 18 ms x
 *         prescaler, §17.0 #31) on a real target.
 */
void EPIC_WDT_Refresh(void);

/**
 * @brief  Enter Sleep (`sleep` asm on target, no-op on host; callers
 *         should keep driving the family sim_step()). Real target
 *         halts until any enabled interrupt wakes it (§14.14).
 */
void EPIC_Sleep_Enter(void);
#if PIC14MIDRANGE_HAS_PCON

/**
 * @brief  Returns 1 if the last reset was a Brown-out Reset
 *         (PCON<BOR>).  Clear after reading via @ref EPIC_BOR_ClearFlag.
 * @return 1 if the last reset was a Brown-out Reset, 0 otherwise.
 */
uint8_t EPIC_BOR_GetStatus(void);

/**
 * @brief Clear PCON<BOR>. The POR bit is write-1-to-clear.
 */
void EPIC_BOR_ClearFlag(void);

/**
 * @brief  Returns 1 if the device just powered on (PCON<POR>).
 *         Set only on Power-on Reset.
 * @return 1 if the last reset was a Power-on Reset, 0 otherwise.
 */
uint8_t EPIC_POR_GetStatus(void);

/**
 * @brief Clear PCON<POR>.
 */
void EPIC_POR_ClearFlag(void);
#endif /* PIC14MIDRANGE_HAS_PCON */

#if PIC14MIDRANGE_HAS_WDT_SW
/**
 * @brief  Enable or disable the software Watchdog Timer (WDTCON<SWDTEN>).
 *         Only effective when the WDTE configuration bit is 0
 *         (DS40001291H §14.5, Register 14-3).
 * @param enable 1 to turn the WDT on, 0 to turn it off.
 */
void EPIC_WDT_SetSoftwareEnable(uint8_t enable);

/**
 * @brief  Set the WDT period prescaler (WDTCON<WDTPS3:WDTPS0>,
 *         DS40001291H Register 14-3). The WDT time base is the
 *         31 kHz LFINTOSC; the nominal time-out is 4 ms times the
 *         prescale ratio (1:32..1:65536, 1 ms..268 s).
 * @param wdtps the 4-bit WDTPS value (0..11; 12..15 reserved).
 */
void EPIC_WDT_SetPrescaler(uint8_t wdtps);
#endif

#endif /* PIC14_WDT_SLEEP_H */
