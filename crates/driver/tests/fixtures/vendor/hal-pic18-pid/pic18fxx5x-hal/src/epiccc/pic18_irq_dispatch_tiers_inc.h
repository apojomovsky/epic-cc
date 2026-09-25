/* epic-cc dispatch tiers for PIC18Fxx5x: one shared body via
 * EPICCC_IRQ_* gates, mirroring pic14-midrange-core's
 * pic16_irq_dispatch_tiers_inc.h. A gated-out source is not in the
 * including tier's HAL subset and cannot vector (PIE off). Each tier
 * is a thin .c wrapper that #defines its gates and #includes this
 * header, so a combo picks a tier by filename without force-linking
 * unused handlers into every other tier's consumer. */

#include "core/pic18_irq.h"

#ifndef EPICCC_IRQ_TMR0
#define EPICCC_IRQ_TMR0 0
#endif
#ifndef EPICCC_IRQ_TMR2
#define EPICCC_IRQ_TMR2 0
#endif
#ifndef EPICCC_IRQ_USART
#define EPICCC_IRQ_USART 0
#endif
#ifndef EPICCC_IRQ_EE
#define EPICCC_IRQ_EE 0
#endif

#if EPICCC_IRQ_TMR0
/** @brief Timer0 overflow IRQ handler. */
extern void TIMER0_IRQHandler(void);
#endif
#if EPICCC_IRQ_TMR2
/** @brief Timer2 match IRQ handler. */
extern void TIMER2_IRQHandler(void);
#endif
#if EPICCC_IRQ_USART
/** @brief USART TX shift-done IRQ handler. */
extern void USART_TX_IRQHandler(void);
/** @brief USART RX byte-ready IRQ handler. */
extern void USART_RX_IRQHandler(void);
#endif
#if EPICCC_IRQ_EE
/** @brief EEPROM write-complete IRQ handler. */
extern void EEPROM_IRQHandler(void);
#endif

/**
 * @brief  Dispatch the tier's pending interrupt sources under the same
 *         per-source gating rules as the full fan-out
 *         (core/pic18_irq_dispatch.c).
 */
void epic_dispatch_all_irqs(void)
{
#if EPICCC_IRQ_TMR0
    /* TMR0IF is dispatched unconditionally on the flag, same as the
     * full fan-out: TIMER0_IRQHandler re-checks and clears its own
     * flag, and nothing in this tier free-runs Timer0 with its
     * overflow IRQ disabled, so there is no stale-flag case to gate
     * on TMR0IE (unlike TMR1 elsewhere in the full fan-out). */
    uint8_t intcon = epic_sfr_read8(PIC_REG_INTCON);
    if (intcon & PIC_INTCON_TMR0IF) TIMER0_IRQHandler();
#endif
#if EPICCC_IRQ_TMR2 || EPICCC_IRQ_USART
    uint8_t pir1 = epic_sfr_read8(PIC_REG_PIR1);
#if EPICCC_IRQ_TMR2
    if (pir1 & PIC_PIR1_TMR2IF) TIMER2_IRQHandler();
#endif
#if EPICCC_IRQ_USART
    /* TX dispatches only under TXIE: TXIF is a status bit that stays
     * set while TXREG is empty. */
    if (pir1 & PIC_PIR1_TXIF)
    {
        if (epic_sfr_read8(PIC_REG_PIE1) & PIC_PIE1_TXIE)
        {
            USART_TX_IRQHandler();
        }
    }
    if (pir1 & PIC_PIR1_RCIF) USART_RX_IRQHandler();
#endif
#endif
#if EPICCC_IRQ_EE
    /* EEIF is left to its polling consumer when EEIE is off. */
    uint8_t pir2 = epic_sfr_read8(PIC_REG_PIR2);
    if (pir2 & PIC_PIR2_EEIF)
    {
        if (epic_sfr_read8(PIC_REG_PIE2) & PIC_PIE2_EEIE)
        {
            EEPROM_IRQHandler();
        }
    }
#endif
}
