/* epic-cc implementation of EPIC_WDT_Refresh / EPIC_Sleep_Enter. */

#include "core/pic18fxx5x_wdt_sleep.h"

#ifdef __has_include
#if __has_include(<epic-cc.h>)
#include <epic-cc.h>
#endif
#endif

#ifndef __epic_clrwdt
#define __epic_clrwdt() asm volatile("clrwdt")
#endif
#ifndef __epic_sleep
#define __epic_sleep() asm volatile("sleep")
#endif

/**
 * @brief Refresh the Watchdog Timer (epic-cc build) via `clrwdt`.
 */
void EPIC_WDT_Refresh(void)
{
    __epic_clrwdt();
}

/**
 * @brief Enter Sleep via `sleep`; the CPU halts until an interrupt wakes it.
 */
void EPIC_Sleep_Enter(void)
{
    __epic_sleep();
}
