/* Shared PIC14 mid-range epic-cc dispatch: Timer0, RB and Timer2 (the
 * scheduling core's sources), other flags cleared (the full fan-out
 * pulls every peripheral handler into the slice; the rest hit filed
 * isel gaps). The tick's 1 ms timebase registers a Timer2 overflow
 * callback, which must fire for the tick to advance (epic-hal#86). */

#include "core/pic16_irq.h"
#include "pic14_midrange.h"

/* The PIR2 EEPROM block reads EEIF/EEIE; existing families (87XA/88X)
 * define no HAS_EE_PIR2 and carry PIR2 EEPROM flags, so default to 1.
 * The 7x (no EEPROM at all) sets it 0 in its shim to compile the block
 * out. */
#ifndef PIC14MIDRANGE_HAS_EE_PIR2
#define PIC14MIDRANGE_HAS_EE_PIR2 1
#endif

/** @brief TIMER0_IRQHandler (weak).
 */
extern void TIMER0_IRQHandler(void);
/** @brief RB_IRQHandler (weak).
 */
extern void RB_IRQHandler(void);
#if PIC14MIDRANGE_HAS_TMR2
/** @brief TIMER2_IRQHandler (weak).
 */
extern void TIMER2_IRQHandler(void);
#endif

/**
 * @brief Dispatch all pending IRQs.
 */
void epic_dispatch_all_irqs(void)
{
    uint8_t intcon = EPIC_REG8(PIC_REG_INTCON);
    if (intcon & PIC_INTCON_TMR0IF) TIMER0_IRQHandler();
    if (intcon & PIC_INTCON_RBIF) RB_IRQHandler();
    /* Other PIR1/PIR2 flags are not used by the smoke; if any are set,
     * just clear them so they do not re-trigger, without calling the
     * handlers that would pull in the rest of the HAL. On PIR-less
     * families (83/84/84A) the whole PIR1 section compiles out and the
     * INTCON-resident EEIF gets the same clear treatment. */
#if PIC14MIDRANGE_HAS_PIR1
    uint8_t pir1 = EPIC_REG8(PIC_REG_PIR1);
    if (pir1 & PIC_PIR1_TMR1IF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR1IF);
    /* The TMR2/CCP1 rows carry the twin's HAS_ guards: a die without
     * the peripheral defines no PIR1 token for it. CCP1 stays
     * clear-only, unlike the twin: no epiccc slice links pic14_ccp.c,
     * so dispatching would grow every slice or call an unlinked
     * handler. */
#if PIC14MIDRANGE_HAS_TMR2
    if (pir1 & PIC_PIR1_TMR2IF) TIMER2_IRQHandler();
#endif
#if PIC14MIDRANGE_HAS_CCP1
    if (pir1 & PIC_PIR1_CCP1IF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_CCP1IF);
#endif
#if PIC14MIDRANGE_HAS_SSP
    if (pir1 & PIC_PIR1_SSPIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_SSPIF);
#endif
#if PIC14MIDRANGE_HAS_USART
    if (pir1 & PIC_PIR1_RCIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_RCIF);
    if (pir1 & PIC_PIR1_TXIF)
    {
        uint8_t txie;
        EPIC_PIE1_READ_TXIE(txie);
        if (!(txie & PIC_PIE1_TXIE)) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TXIF);
    }
#endif
#if PIC14MIDRANGE_HAS_ADC
    if (pir1 & PIC_PIR1_ADIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_ADIF);
#endif
#if PIC14MIDRANGE_HAS_PSP
    if (pir1 & PIC_PIR1_PSPIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_PSPIF);
#endif
#if PIC14MIDRANGE_HAS_CM_PIR1
    if (pir1 & PIC_PIR1_CMIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_CMIF);
#endif
#if PIC14MIDRANGE_HAS_EE_PIR1
    if (pir1 & PIC_PIR1_EEIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_EEIF);
#endif
#else
    {   /* PIR-less family (83/84/84A): EEIF is EECON1<4>, Bank 1. */
        uint8_t eecon1 = 0U;
        EPIC_BANK1_READ8(EECON1, eecon1);
        if (eecon1 & PIC_EECON1_EEIF) EPIC_BANK1_WRITE8(EECON1, (uint8_t)(eecon1 & (uint8_t)~PIC_EECON1_EEIF));
    }
#endif
#if PIC14MIDRANGE_HAS_PIR2
    /* 87XA PIR2 (DS39582B §14.11): CCP2IF/BCLIF/EEIF/CMIF; the 88X
     * C1/C2/ULPWU/OSF flags do not exist there, and vice versa. */
    uint8_t pir2 = EPIC_REG8(PIC_REG_PIR2);
#if PIC14MIDRANGE_HAS_CCP2
    if (pir2 & PIC_PIR2_CCP2IF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_CCP2IF);
#endif
#if PIC14MIDRANGE_HAS_BCL_DISPATCH
    if (pir2 & PIC_PIR2_BCLIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_BCLIF);
#endif
#if PIC14MIDRANGE_HAS_EE_PIR2
    if (pir2 & PIC_PIR2_EEIF)
    {
        uint8_t eeie = 0u;
        EPIC_PIE2_READ_EEIE(eeie);
        if (!(eeie & PIC_PIE2_EEIE)) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_EEIF);
    }
#endif
#if PIC14MIDRANGE_HAS_COMP_DUAL
    if (pir2 & PIC_PIR2_C1IF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_C1IF);
    if (pir2 & PIC_PIR2_C2IF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_C2IF);
#if PIC14MIDRANGE_HAS_ULPWU
    if (pir2 & PIC_PIR2_ULPWUIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_ULPWUIF);
#endif
#if PIC14MIDRANGE_HAS_OSF
    if (pir2 & PIC_PIR2_OSFIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_OSFIF);
#endif
#elif PIC14MIDRANGE_HAS_CM_PIR2
    if (pir2 & PIC_PIR2_CMIF) EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_CMIF);
#endif
#endif
}
