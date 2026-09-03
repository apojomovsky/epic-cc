/* CPU-level helpers: Watchdog Timer, Brown-out Reset, Sleep. Source:
 * DS39582B §14.10 (PCON), §14.13 (WDT), §14.14 (Sleep). Config bits
 * are left to the user's MPLAB X/XC8 setup; EPIC_Sleep_Enter is a
 * no-op on host. */

#ifndef PIC16F87XA_WDT_SLEEP_H
#define PIC16F87XA_WDT_SLEEP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief  Refresh the Watchdog Timer (`clrwdt`, no-op on host). Must be
 *         called more often than the WDT period (typ. 18 ms x
 *         prescaler, §17.0 #31) on a real target.
 */
void EPIC_WDT_Refresh(void);

/**
 * @brief  Enter Sleep (`sleep` asm on target, no-op on host; callers
 *         should keep driving pic16f87xa_sim_step()). Real target
 *         halts until any enabled interrupt wakes it (§14.14).
 */
void EPIC_Sleep_Enter(void);

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

#endif /* PIC16F87XA_WDT_SLEEP_H */
