/* Vendored epic-cc dispatch fan-out (the real HAL's
 * pic16_irq_dispatch_epiccc.c): Timer0 + RB only, other flags cleared
 * (the full fan-out pulls every peripheral handler into the slice). */

#include "hal_pic16.h"

void TIMER0_IRQHandler(void);
void RB_IRQHandler(void);

void epic_dispatch_all_irqs(void)
{
    uint8_t intcon = EPIC_REG8(PIC_REG_INTCON);
    if (intcon & PIC_INTCON_TMR0IF) TIMER0_IRQHandler();
    if (intcon & PIC_INTCON_RBIF) RB_IRQHandler();
    /* Other PIR1/PIR2 flags are not used by the slice; if any are set,
     * just clear them so they do not re-trigger. */
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), EPIC_BIT(0)); /* TMR1IF */
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), EPIC_BIT(1)); /* TMR2IF */
}
