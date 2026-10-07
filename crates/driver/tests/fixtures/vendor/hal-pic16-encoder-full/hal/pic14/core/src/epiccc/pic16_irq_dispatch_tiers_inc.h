/* epic-cc dispatch tiers: one shared body per module class via
 * EPICCC_IRQ_* gates. Only gated sources dispatch; others are not in
 * the HAL subset and cannot vector (PIE off), so no clear scaffolding.
 * The full fan-out pulls every handler into the program, and 368-byte
 * GPR cannot pay for the unused ones. */

#include "core/pic16_irq.h"
#include "pic14_midrange.h"

#ifndef EPICCC_IRQ_TMR0
#define EPICCC_IRQ_TMR0 0
#endif
#ifndef EPICCC_IRQ_TMR1
#define EPICCC_IRQ_TMR1 0
#endif
#ifndef EPICCC_IRQ_TMR2
#define EPICCC_IRQ_TMR2 0
#endif
#ifndef EPICCC_IRQ_RB
#define EPICCC_IRQ_RB 0
#endif
#ifndef EPICCC_IRQ_USART
#define EPICCC_IRQ_USART 0
#endif
#ifndef EPICCC_IRQ_SSP
#define EPICCC_IRQ_SSP 0
#endif
#ifndef EPICCC_IRQ_ADC
#define EPICCC_IRQ_ADC 0
#endif
#ifndef EPICCC_IRQ_EE
#define EPICCC_IRQ_EE 0
#endif
#ifndef EPICCC_IRQ_CCP1
#define EPICCC_IRQ_CCP1 0
#endif
#ifndef EPICCC_IRQ_CCP2
#define EPICCC_IRQ_CCP2 0
#endif

#if EPICCC_IRQ_TMR1
/** @brief Timer1 IRQ handler. */
extern void TIMER1_IRQHandler(void);
#endif
#if EPICCC_IRQ_TMR2
/** @brief Timer2 IRQ handler. */
extern void TIMER2_IRQHandler(void);
#endif
#if EPICCC_IRQ_CCP1
/** @brief CCP1 IRQ handler. */
extern void CCP1_IRQHandler(void);
#endif
#if EPICCC_IRQ_CCP2
/** @brief CCP2 IRQ handler. */
extern void CCP2_IRQHandler(void);
#endif
#if EPICCC_IRQ_USART
/** @brief USART RX IRQ handler. */
extern void USART_RX_IRQHandler(void);
/** @brief USART TX IRQ handler. */
extern void USART_TX_IRQHandler(void);
#endif
#if EPICCC_IRQ_TMR0
/** @brief Timer0 IRQ handler. */
extern void TIMER0_IRQHandler(void);
#endif
#if EPICCC_IRQ_RB
/** @brief RB port-change IRQ handler. */
extern void RB_IRQHandler(void);
#endif
#if EPICCC_IRQ_SSP
/** @brief MSSP IRQ handler. */
extern void SSP_IRQHandler(void);
#endif
#if EPICCC_IRQ_ADC
/** @brief ADC conversion-done IRQ handler. */
extern void ADC_IRQHandler(void);
#endif
#if EPICCC_IRQ_EE
/** @brief EEPROM write-complete IRQ handler. */
extern void EEPROM_IRQHandler(void);
#endif

/**
 * @brief Dispatch the tier's pending interrupt sources.
 */
void epic_dispatch_all_irqs(void)
{
#if EPICCC_IRQ_TMR0
    if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IF)
    {
        if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IE) TIMER0_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
    }
#endif
#if EPICCC_IRQ_TMR1
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR1IF)
    {
        uint8_t tmr1ie; EPIC_PIE1_READ_TMR1IE(tmr1ie);
        if (tmr1ie & PIC_PIE1_TMR1IE)
        {
            TIMER1_IRQHandler();
        }
        else
        {
            EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR1IF);
        }
    }
#endif
#if EPICCC_IRQ_RB
    if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_RBIF)
    {
        if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_RBIE) RB_IRQHandler();
        else
        {
            (void)EPIC_REG8(PIC_REG_PORTB); EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_RBIF);
        }
    }
#endif
#if EPICCC_IRQ_SSP
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_SSPIF)
    {
        uint8_t sspie; EPIC_PIE1_READ_SSPIE(sspie);
        if (sspie & PIC_PIE1_SSPIE) SSP_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_SSPIF);
    }
#endif
#if EPICCC_IRQ_ADC
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_ADIF)
    {
        uint8_t adie; EPIC_PIE1_READ_ADIE(adie);
        if (adie & PIC_PIE1_ADIE) ADC_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_ADIF);
    }
#endif
#if EPICCC_IRQ_EE
#if PIC14MIDRANGE_HAS_PIR2
    if (EPIC_REG8(PIC_REG_PIR2) & PIC_PIR2_EEIF)
    {
        uint8_t eeie; EPIC_PIE2_READ_EEIE(eeie);
        if (eeie & PIC_PIE2_EEIE) EEPROM_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_EEIF);
    }
#else
    /* PIR-less family (83/84/84A): the gate EEIE is INTCON<6>, the
     * flag EEIF is EECON1<4>, Bank 1 (literal-token access). */
    if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_EEIE)
    {
        uint8_t eecon1 = 0U;
        EPIC_BANK1_READ8(EECON1, eecon1);
        if (eecon1 & PIC_EECON1_EEIF) EEPROM_IRQHandler();
        else EPIC_BANK1_WRITE8(EECON1, (uint8_t)(eecon1 & (uint8_t)~PIC_EECON1_EEIF));
    }
#endif
#endif
#if EPICCC_IRQ_TMR2
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR2IF)
    {
        uint8_t tmr2ie; EPIC_PIE1_READ_TMR2IE(tmr2ie);
        if (tmr2ie & PIC_PIE1_TMR2IE) TIMER2_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
    }
#endif
#if EPICCC_IRQ_CCP1
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_CCP1IF)
    {
        uint8_t ccp1ie; EPIC_PIE1_READ_CCP1IE(ccp1ie);
        if (ccp1ie & PIC_PIE1_CCP1IE) CCP1_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_CCP1IF);
    }
#endif
#if EPICCC_IRQ_USART
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_RCIF) USART_RX_IRQHandler();
    if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TXIF)
    {
        uint8_t txie; EPIC_PIE1_READ_TXIE(txie);
        if (txie & PIC_PIE1_TXIE) USART_TX_IRQHandler();
    }
#endif
#if EPICCC_IRQ_CCP2
    if (EPIC_REG8(PIC_REG_PIR2) & PIC_PIR2_CCP2IF)
    {
        uint8_t ccp2ie; EPIC_PIE2_READ_CCP2IE(ccp2ie);
        if (ccp2ie & PIC_PIE2_CCP2IE) CCP2_IRQHandler();
        else EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_CCP2IF);
    }
#endif
}
