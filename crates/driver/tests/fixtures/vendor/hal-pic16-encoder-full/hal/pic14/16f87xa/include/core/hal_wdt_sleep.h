/* Family-neutral WDT / Sleep / BOR / POR contract (EPIC_WDT_*).
 * Implementation lives in the shared pic14-midrange-core; this header
 * redirects to it so family-neutral consumers (epic-taskmgr) keep
 * building unchanged across families. */

#ifndef EPIC_WDT_SLEEP_H
#define EPIC_WDT_SLEEP_H
#include "core/pic14_wdt_sleep.h"
#endif /* EPIC_WDT_SLEEP_H */
