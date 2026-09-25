/* epic-cc dispatch, TMR0-tick + serial + TMR2 + EEPROM tier: the same
 * serial+TMR2+EEPROM tier as pic18_irq_dispatch_epiccc.c, plus TIMER0
 * overflow (the epic-taskmgr scheduler's tick source, wired via
 * epic_taskmgr_attach_timer0 -> EPIC_TIMER0_Init/Start). Sources
 * outside this tier's gates are not in the HAL subset and cannot
 * vector (PIE off). Thin wrapper over the shared tiers body; see
 * pic18_irq_dispatch_tiers_inc.h for the dispatch code and gating
 * semantics (epic-hal#237). */

#define EPICCC_IRQ_TMR0  1
#define EPICCC_IRQ_TMR2  1
#define EPICCC_IRQ_USART 1
#define EPICCC_IRQ_EE    1
#include "pic18_irq_dispatch_tiers_inc.h"
