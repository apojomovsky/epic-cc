/* epic-cc dispatch, serial + scheduling tier (epic-modbus: serial plus
 * the tick's Timer2 timebase): USART RX/TX and TIMER2 dispatch, the
 * rest cleared. */

#define EPICCC_IRQ_USART 1
#define EPICCC_IRQ_TMR2 1
#include "pic16_irq_dispatch_tiers_inc.h"
