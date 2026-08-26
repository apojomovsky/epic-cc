/* Vendored epic-cc interrupt-vector entry: the single PIC16 vector at
 * 0x0004 (DS39582B §14.11) delegates to the shared dispatcher
 * (pic16_irq_dispatch.c in the real HAL). */

#include "hal_pic16.h"

void epic_dispatch_all_irqs(void);

void __attribute__((interrupt(0))) PIC16_IRQ_Handler(void)
{
    epic_dispatch_all_irqs();
}
