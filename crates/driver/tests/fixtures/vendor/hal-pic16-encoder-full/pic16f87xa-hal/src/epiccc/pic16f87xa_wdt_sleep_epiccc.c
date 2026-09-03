/* epic-cc implementation of EPIC_WDT_Refresh / EPIC_Sleep_Enter.
 * Uses the intrinsics from <epic-cc.h> (which lower to opaque asm
 * blocks the compiler understands). The XC8 target uses raw asm()
 * clrwdt/sleep; the host sim no-ops. */

#include "core/pic16f87xa_wdt_sleep.h"

#ifdef __has_include
#if __has_include(<epic-cc.h>)
#include <epic-cc.h>
#endif
#endif

/* Fall back when building the file with host gcc for a smoke check. */
#ifndef __epic_clrwdt
#define __epic_clrwdt() asm volatile("clrwdt")
#endif
#ifndef __epic_sleep
#define __epic_sleep() asm volatile("sleep")
#endif

/**
 * @brief Refresh the Watchdog Timer with the native `clrwdt` instruction.
 */
void EPIC_WDT_Refresh(void)
{
    __epic_clrwdt();
}

/**
 * @brief Enter Sleep with the native `sleep` instruction.
 */
void EPIC_Sleep_Enter(void)
{
    __epic_sleep();
}
