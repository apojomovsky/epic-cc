/* epic-cc interrupt-vector entry: the single PIC16 vector at 0x0004
 * (DS39582B 14.11) delegates to the shared dispatcher. The XC8 target
 * uses its interrupt attribute plus inline asm to normalize the bank;
 * epic-cc uses __attribute__((interrupt(0))) and its banking pass
 * handles the rest. */

#include "core/pic16_irq.h"

/**
 * @brief Fan-out dispatcher invoked from the interrupt vector; defined
 *        in pic16_irq_dispatch.c.
 */
extern void epic_dispatch_all_irqs(void);

/**
 * @brief Single PIC16 interrupt-vector entry, delegates to
 *        @ref epic_dispatch_all_irqs.
 */
void __attribute__((interrupt(0))) PIC16_IRQ_Handler(void)
{
    epic_dispatch_all_irqs();
}
