/*
 * CPU-level helpers: Watchdog Timer, Brown-out Reset, Sleep. Matches
 * `pic14_wdt_sleep.h`'s API (DS39632E §4.0/§9.0/§14.x/§3.0); PIC18
 * folds the reset-status bits (TO/PD/POR/BOR) into RCON instead of PIC16's
 * separate PCON. `EPIC_WDT_Refresh`/`EPIC_Sleep_Enter` are link-time-
 * selected (`*_sim.c` host, `*_target.c` XC8).
 */

#ifndef PIC18FXX5X_WDT_SLEEP_H
#define PIC18FXX5X_WDT_SLEEP_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief  Refresh the Watchdog Timer by executing the `clrwdt` asm
 *         instruction (or the equivalent no-op in the simulator).
 *
 *         On a real target this MUST be called more often than the WDT
 *         period (DS39632E §14.x). On the host simulator it's a no-op.
 */
void EPIC_WDT_Refresh(void);

/**
 * @brief  Enter Power-down (Sleep) mode. On a real target this is the
 *         `sleep` asm instruction; the CPU halts until any enabled
 *         interrupt wakes it (DS39632E §3.0).
 *
 *         On the host simulator this is a no-op; callers should continue
 *         to drive pic18_sim_step().
 */
void EPIC_Sleep_Enter(void);

/**
 * @brief  Returns 1 if the last reset was a Brown-out Reset
 *         (RCON<BOR>). Clear after reading via @ref EPIC_BOR_ClearFlag.
 * @return 1 if the last reset was a Brown-out Reset, else 0.
 */
uint8_t EPIC_BOR_GetStatus(void);

/**
 * @brief Clear RCON<BOR> (write 0).
 */
void EPIC_BOR_ClearFlag(void);

/**
 * @brief  Returns 1 if the device just powered on (RCON<POR>).
 *         Set on Power-on Reset (DS39632E Register 4-1).
 * @return 1 if the device just powered on, else 0.
 */
uint8_t EPIC_POR_GetStatus(void);

/**
 * @brief Clear RCON<POR> (write 0).
 */
void EPIC_POR_ClearFlag(void);

#endif /* PIC18FXX5X_WDT_SLEEP_H */
