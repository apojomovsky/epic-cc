/*
 * IRQ driver implementation. Every function names its SFR as a
 * compile-time-constant `PIC_REG_*` token, never a runtime address: on
 * PIC18 a runtime SFR address compiles to the program-memory table
 * mechanism instead of a data access (see
 * `pic18fxx5x-hal/README.md`). GIEH/GIEL (INTCON<7:6>) act as
 * one on/off switch; enabling also sets IPEN (RCON<7>) for the
 * two-vector priority scheme.
 */

#include "core/pic18_irq.h"

/* `reg` must be a literal `PIC_REG_*` token so it stays a compile-time
 * constant through `epic_sfr_read8`/`write8` (see file header). */
#define SFR_SET_BIT(reg, mask) \
    epic_sfr_write8((reg), (uint8_t)(epic_sfr_read8(reg) | (mask)))
#define SFR_CLR_BIT(reg, mask) \
    epic_sfr_write8((reg), (uint8_t)(epic_sfr_read8(reg) & (uint8_t)~(mask)))

/**
 * @brief  Globally mask all interrupts by clearing the master enable(s)
 *         (INTCON<GIEH/GIEL>, DS39632E §9.0). In priority mode both GIEH
 *         and GIEL are cleared.
 * @return 1 if any master enable was set (interrupts were on), else 0.
 */
uint8_t EPIC_IRQ_Disable(void)
{
    uint8_t intcon = epic_sfr_read8(PIC_REG_INTCON);
    uint8_t prev = (intcon & (PIC_INTCON_GIEH | PIC_INTCON_GIEL)) ? 1U : 0U;
    SFR_CLR_BIT(PIC_REG_INTCON, PIC_INTCON_GIEH);
    SFR_CLR_BIT(PIC_REG_INTCON, PIC_INTCON_GIEL);
    return prev;
}

/**
 * @brief  Restore the master interrupt enable(s). `prev_state` is the
 *         value returned by @ref EPIC_IRQ_Disable. Restoring to "on"
 *         also ensures IPEN = 1 (priority mode) so the two-vector scheme
 *         is active.
 * @param prev_state 1 to enable all interrupts, 0 to keep them masked.
 */
void EPIC_IRQ_Restore(uint8_t prev_state)
{
    if (prev_state)
    {
        /* Activate the two-vector priority scheme (DS39632E §9.0, RCON<7>)
         * before enabling the masters, so high/low routing is in effect. */
        SFR_SET_BIT(PIC_REG_RCON, PIC_RCON_IPEN);
        SFR_SET_BIT(PIC_REG_INTCON, PIC_INTCON_GIEH);
        SFR_SET_BIT(PIC_REG_INTCON, PIC_INTCON_GIEL);
    }
    else
    {
        SFR_CLR_BIT(PIC_REG_INTCON, PIC_INTCON_GIEH);
        SFR_CLR_BIT(PIC_REG_INTCON, PIC_INTCON_GIEL);
    }
}

/**
 * @brief  Enable one interrupt source. The peripheral enable bit lives in
 *         INTCON / INTCON3 / PIE1 per the source. The master enable(s)
 *         must still be set via @ref EPIC_IRQ_Restore for the source to
 *         fire.
 * @param irq the interrupt source to enable.
 */
void EPIC_IRQ_Enable(PIC18_IRQn irq)
{
    switch (irq)
    {
    case PIC18_IRQ_INT0:     SFR_SET_BIT(PIC_REG_INTCON,  PIC_INTCON_INT0IE);  break;
    case PIC18_IRQ_INT1:     SFR_SET_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT1IE); break;
    case PIC18_IRQ_INT2:     SFR_SET_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT2IE); break;
    case PIC18_IRQ_RB:       SFR_SET_BIT(PIC_REG_INTCON,  PIC_INTCON_RBIE);   break;
    case PIC18_IRQ_TMR0:     SFR_SET_BIT(PIC_REG_INTCON,  PIC_INTCON_TMR0IE); break;
    case PIC18_IRQ_TMR1:     SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_TMR1IE);   break;
    case PIC18_IRQ_TMR2:     SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_TMR2IE);   break;
    case PIC18_IRQ_TMR3:     SFR_SET_BIT(PIC_REG_PIE2,    PIC_PIE2_TMR3IE);   break;
    case PIC18_IRQ_CCP1:     SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_CCP1IE);   break;
    case PIC18_IRQ_SSP:      SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_SSPIE);    break;
    case PIC18_IRQ_USART_TX: SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_TXIE);     break;
    case PIC18_IRQ_USART_RX: SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_RCIE);     break;
    case PIC18_IRQ_ADC:      SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_ADIE);     break;
    case PIC18_IRQ_CCP2:     SFR_SET_BIT(PIC_REG_PIE2,    PIC_PIE2_CCP2IE);   break;
    case PIC18_IRQ_CMP:      SFR_SET_BIT(PIC_REG_PIE2,    PIC_PIE2_CMIE);     break;
    case PIC18_IRQ_EEPROM:   SFR_SET_BIT(PIC_REG_PIE2,    PIC_PIE2_EEIE);     break;
#if PIC18FXX5X_FAMILY_HAS_SPP
    case PIC18_IRQ_SPP:      SFR_SET_BIT(PIC_REG_PIE1,    PIC_PIE1_SPPIE);    break;
#endif
    default: break;
    }
}

/**
 * @brief  Disable one interrupt source. The peripheral enable bit lives
 *         in INTCON / INTCON3 / PIE1 per the source.
 * @param irq the interrupt source to disable.
 */
void EPIC_IRQ_DisableSrc(PIC18_IRQn irq)
{
    switch (irq)
    {
    case PIC18_IRQ_INT0:     SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_INT0IE);  break;
    case PIC18_IRQ_INT1:     SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT1IE); break;
    case PIC18_IRQ_INT2:     SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT2IE); break;
    case PIC18_IRQ_RB:       SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_RBIE);   break;
    case PIC18_IRQ_TMR0:     SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_TMR0IE); break;
    case PIC18_IRQ_TMR1:     SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_TMR1IE);   break;
    case PIC18_IRQ_TMR2:     SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_TMR2IE);   break;
    case PIC18_IRQ_TMR3:     SFR_CLR_BIT(PIC_REG_PIE2,    PIC_PIE2_TMR3IE);   break;
    case PIC18_IRQ_CCP1:     SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_CCP1IE);   break;
    case PIC18_IRQ_SSP:      SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_SSPIE);    break;
    case PIC18_IRQ_USART_TX: SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_TXIE);     break;
    case PIC18_IRQ_USART_RX: SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_RCIE);     break;
    case PIC18_IRQ_ADC:      SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_ADIE);     break;
    case PIC18_IRQ_CCP2:     SFR_CLR_BIT(PIC_REG_PIE2,    PIC_PIE2_CCP2IE);   break;
    case PIC18_IRQ_CMP:      SFR_CLR_BIT(PIC_REG_PIE2,    PIC_PIE2_CMIE);     break;
    case PIC18_IRQ_EEPROM:   SFR_CLR_BIT(PIC_REG_PIE2,    PIC_PIE2_EEIE);     break;
#if PIC18FXX5X_FAMILY_HAS_SPP
    case PIC18_IRQ_SPP:      SFR_CLR_BIT(PIC_REG_PIE1,    PIC_PIE1_SPPIE);    break;
#endif
    default: break;
    }
}

/**
 * @brief  Clear the interrupt flag of `irq`. MUST be called inside the
 *         ISR before re-enabling interrupts to avoid an infinite
 *         re-entry (DS39632E §9.0).
 * @param irq the interrupt source whose flag is cleared.
 */
void EPIC_IRQ_ClearFlag(PIC18_IRQn irq)
{
    switch (irq)
    {
    case PIC18_IRQ_INT0:     SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_INT0IF);  break;
    case PIC18_IRQ_INT1:     SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT1IF); break;
    case PIC18_IRQ_INT2:     SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT2IF); break;
    case PIC18_IRQ_RB:       SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_RBIF);   break;
    case PIC18_IRQ_TMR0:     SFR_CLR_BIT(PIC_REG_INTCON,  PIC_INTCON_TMR0IF); break;
    case PIC18_IRQ_TMR1:     SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_TMR1IF);   break;
    case PIC18_IRQ_TMR2:     SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_TMR2IF);   break;
    case PIC18_IRQ_TMR3:     SFR_CLR_BIT(PIC_REG_PIR2,    PIC_PIR2_TMR3IF);   break;
    case PIC18_IRQ_CCP1:     SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_CCP1IF);   break;
    case PIC18_IRQ_SSP:      SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_SSPIF);    break;
    case PIC18_IRQ_USART_TX: SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_TXIF);     break;
    case PIC18_IRQ_USART_RX: SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_RCIF);     break;
    case PIC18_IRQ_ADC:      SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_ADIF);     break;
    case PIC18_IRQ_CCP2:     SFR_CLR_BIT(PIC_REG_PIR2,    PIC_PIR2_CCP2IF);   break;
    case PIC18_IRQ_CMP:      SFR_CLR_BIT(PIC_REG_PIR2,    PIC_PIR2_CMIF);     break;
    case PIC18_IRQ_EEPROM:   SFR_CLR_BIT(PIC_REG_PIR2,    PIC_PIR2_EEIF);     break;
#if PIC18FXX5X_FAMILY_HAS_SPP
    case PIC18_IRQ_SPP:      SFR_CLR_BIT(PIC_REG_PIR1,    PIC_PIR1_SPPIF);    break;
#endif
    default: break;
    }
}

/**
 * @brief  Return the current pending state of `irq`.
 * @param irq the interrupt source to query.
 * @return 1 if the source's flag is set (pending), else 0.
 */
uint8_t EPIC_IRQ_GetFlag(PIC18_IRQn irq)
{
    switch (irq)
    {
    case PIC18_IRQ_INT0:     return (epic_sfr_read8(PIC_REG_INTCON)  & PIC_INTCON_INT0IF)  ? 1U : 0U;
    case PIC18_IRQ_INT1:     return (epic_sfr_read8(PIC_REG_INTCON3) & PIC_INTCON3_INT1IF) ? 1U : 0U;
    case PIC18_IRQ_INT2:     return (epic_sfr_read8(PIC_REG_INTCON3) & PIC_INTCON3_INT2IF) ? 1U : 0U;
    case PIC18_IRQ_RB:       return (epic_sfr_read8(PIC_REG_INTCON)  & PIC_INTCON_RBIF)    ? 1U : 0U;
    case PIC18_IRQ_TMR0:     return (epic_sfr_read8(PIC_REG_INTCON)  & PIC_INTCON_TMR0IF)  ? 1U : 0U;
    case PIC18_IRQ_TMR1:     return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_TMR1IF)    ? 1U : 0U;
    case PIC18_IRQ_TMR2:     return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_TMR2IF)    ? 1U : 0U;
    case PIC18_IRQ_TMR3:     return (epic_sfr_read8(PIC_REG_PIR2)    & PIC_PIR2_TMR3IF)    ? 1U : 0U;
    case PIC18_IRQ_CCP1:     return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_CCP1IF)    ? 1U : 0U;
    case PIC18_IRQ_SSP:      return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_SSPIF)     ? 1U : 0U;
    case PIC18_IRQ_USART_TX: return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_TXIF)      ? 1U : 0U;
    case PIC18_IRQ_USART_RX: return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_RCIF)      ? 1U : 0U;
    case PIC18_IRQ_ADC:      return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_ADIF)      ? 1U : 0U;
    case PIC18_IRQ_CCP2:     return (epic_sfr_read8(PIC_REG_PIR2)    & PIC_PIR2_CCP2IF)    ? 1U : 0U;
    case PIC18_IRQ_CMP:      return (epic_sfr_read8(PIC_REG_PIR2)    & PIC_PIR2_CMIF)      ? 1U : 0U;
    case PIC18_IRQ_EEPROM:   return (epic_sfr_read8(PIC_REG_PIR2)    & PIC_PIR2_EEIF)      ? 1U : 0U;
#if PIC18FXX5X_FAMILY_HAS_SPP
    case PIC18_IRQ_SPP:      return (epic_sfr_read8(PIC_REG_PIR1)    & PIC_PIR1_SPPIF)     ? 1U : 0U;
#endif
    default: return 0U;
    }
}

/**
 * @brief  Set the priority of `irq` (high or low vector). Writes the
 *         matching bit in INTCON2 / INTCON3 / IPR1. INT0 has no priority
 *         bit (always high); setting its priority is a no-op. Takes
 *         effect only in priority mode (IPEN = 1, which @ref
 *         EPIC_IRQ_Restore enables).
 * @param irq  the interrupt source.
 * @param prio the desired priority (high or low vector).
 */
void EPIC_IRQ_SetPriority(PIC18_IRQn irq, EPIC_IRQ_Priority prio)
{
    uint8_t high = (prio == EPIC_IRQ_PRIORITY_HIGH) ? 1U : 0U;
    switch (irq)
    {
    case PIC18_IRQ_INT0:     break; /* always high, no bit to set. */
    case PIC18_IRQ_INT1:
        if (high) SFR_SET_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT1IP);
        else      SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT1IP);
        break;
    case PIC18_IRQ_INT2:
        if (high) SFR_SET_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT2IP);
        else      SFR_CLR_BIT(PIC_REG_INTCON3, PIC_INTCON3_INT2IP);
        break;
    case PIC18_IRQ_RB:
        if (high) SFR_SET_BIT(PIC_REG_INTCON2, PIC_INTCON2_RBIP);
        else      SFR_CLR_BIT(PIC_REG_INTCON2, PIC_INTCON2_RBIP);
        break;
    case PIC18_IRQ_TMR0:
        if (high) SFR_SET_BIT(PIC_REG_INTCON2, PIC_INTCON2_TMR0IP);
        else      SFR_CLR_BIT(PIC_REG_INTCON2, PIC_INTCON2_TMR0IP);
        break;
    case PIC18_IRQ_TMR1:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_TMR1IP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_TMR1IP);
        break;
    case PIC18_IRQ_TMR2:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_TMR2IP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_TMR2IP);
        break;
    case PIC18_IRQ_TMR3:
        if (high) SFR_SET_BIT(PIC_REG_IPR2, PIC_IPR2_TMR3IP);
        else      SFR_CLR_BIT(PIC_REG_IPR2, PIC_IPR2_TMR3IP);
        break;
    case PIC18_IRQ_CCP1:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_CCP1IP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_CCP1IP);
        break;
    case PIC18_IRQ_SSP:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_SSPIP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_SSPIP);
        break;
    case PIC18_IRQ_USART_TX:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_TXIP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_TXIP);
        break;
    case PIC18_IRQ_USART_RX:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_RCIP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_RCIP);
        break;
    case PIC18_IRQ_ADC:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_ADIP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_ADIP);
        break;
    case PIC18_IRQ_CCP2:
        if (high) SFR_SET_BIT(PIC_REG_IPR2, PIC_IPR2_CCP2IP);
        else      SFR_CLR_BIT(PIC_REG_IPR2, PIC_IPR2_CCP2IP);
        break;
    case PIC18_IRQ_CMP:
        if (high) SFR_SET_BIT(PIC_REG_IPR2, PIC_IPR2_CMIP);
        else      SFR_CLR_BIT(PIC_REG_IPR2, PIC_IPR2_CMIP);
        break;
    case PIC18_IRQ_EEPROM:
        if (high) SFR_SET_BIT(PIC_REG_IPR2, PIC_IPR2_EEIP);
        else      SFR_CLR_BIT(PIC_REG_IPR2, PIC_IPR2_EEIP);
        break;
#if PIC18FXX5X_FAMILY_HAS_SPP
    case PIC18_IRQ_SPP:
        if (high) SFR_SET_BIT(PIC_REG_IPR1, PIC_IPR1_SPPIP);
        else      SFR_CLR_BIT(PIC_REG_IPR1, PIC_IPR1_SPPIP);
        break;
#endif
    default: break;
    }
}
