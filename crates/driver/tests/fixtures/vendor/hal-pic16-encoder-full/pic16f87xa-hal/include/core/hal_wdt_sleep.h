/* Family-neutral WDT / Sleep / BOR / POR contract. Family-agnostic
 * consumers include this name instead of pic16f87xa_wdt_sleep.h; each
 * family provides its own copy that pulls in its family-specific
 * header. The include path selects which family's copy resolves. */

#ifndef EPIC_WDT_SLEEP_H
#define EPIC_WDT_SLEEP_H
#include "pic16f87xa_wdt_sleep.h"
#endif /* EPIC_WDT_SLEEP_H */
