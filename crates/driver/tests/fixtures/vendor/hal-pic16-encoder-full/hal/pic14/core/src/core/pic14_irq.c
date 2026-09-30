/* Shared PIC14 mid-range interrupt controller implementation (87XA +
 * 88X). Sources: DS39582B §14.11 (87XA), DS40001291H §14.11 (88X). The
 * per-source translation table lives in the family's own
 * src/core/pic16_irq_table.c (its own IRQn enum); this body consumes it
 * through core/pic14_irq_common.h. */

#include "core/pic16_irq.h"
#include "core/pic14_irq_common.h"

/* public API. */

/**
 * @brief Globally mask all interrupts by clearing the GIE bit.
 * @return the previous GIE state (1 = was enabled).
 */
uint8_t EPIC_IRQ_Disable(void)
{
    uint8_t s = EPIC_REG8(PIC_REG_INTCON);
    uint8_t prev = (s & PIC_INTCON_GIE) ? 1U : 0U;
    EPIC_REG8(PIC_REG_INTCON) = s & (uint8_t)~PIC_INTCON_GIE;
    return prev;
}

/**
 * @brief Restore the global interrupt enable to `prev_state`, the pair
 *        of @ref EPIC_IRQ_Disable.
 * @param prev_state the GIE state returned by @ref EPIC_IRQ_Disable.
 */
void EPIC_IRQ_Restore(uint8_t prev_state)
{
    if (prev_state) EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_GIE);
    else            EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_GIE);
}

/**
 * @brief Enable one interrupt source (sets the matching PIE/INTCON bit
 *        and PEIE for peripheral sources).
 * @param irq the interrupt source to enable.
 */
void EPIC_IRQ_Enable(PIC16_IRQn irq)
{
    if ((unsigned)irq >= IRQ_TABLE_SIZE) return;
    const irq_desc_t *d = &irq_table[irq];
    /* irq_table is ROM-resident; XC8 reads each field through a runtime
     * helper, and interleaving that read with an in-progress SFR RMW
     * silently corrupted the SFR side. Pull every needed field into
     * locals before touching any SFR. */
    uint8_t in_intcon = d->enable_in_intcon;
    uint8_t enable_mask = d->enable_mask;
    if (in_intcon)
    {
        EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), enable_mask);
        return;
    }
#if PIC14MIDRANGE_HAS_PIR1
    /* Bank 1 (PIE1/PIE2): a plain C RMW here never persisted under XC8
     * v4.00 (see EPIC_PIE_ENABLE_BIT's own comment). Lives in the
     * per-platform header, not inline here, because this file is
     * shared with the host build and asm()/__at() are XC8-only. */
    EPIC_PIE_ENABLE_BIT(d->pir_is_pir2, enable_mask);
    /* Peripheral IRQs also need PEIE; auto-set it as a courtesy. */
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_PEIE);
#else
    (void)enable_mask;
#endif
}

/**
 * @brief Disable one interrupt source (clears the matching PIE/INTCON bit).
 * @param irq the interrupt source to disable.
 */
void EPIC_IRQ_DisableSrc(PIC16_IRQn irq)
{
    if ((unsigned)irq >= IRQ_TABLE_SIZE) return;
    const irq_desc_t *d = &irq_table[irq];
    uint8_t in_intcon = d->enable_in_intcon;
    uint8_t enable_mask = d->enable_mask;
    if (in_intcon)
    {
        EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), enable_mask);
        return;
    }
#if PIC14MIDRANGE_HAS_PIR1
    /* Same fix as EPIC_IRQ_Enable, see EPIC_PIE_DISABLE_BIT's header
     * comment (the family target platform header) for the full account. */
    EPIC_PIE_DISABLE_BIT(d->pir_is_pir2, enable_mask);
#else
    (void)enable_mask;
#endif
}

/**
 * @brief Clear the interrupt flag of `irq` (PIR/INTCON bit).
 * @param irq the interrupt source whose flag to clear.
 */
void EPIC_IRQ_ClearFlag(PIC16_IRQn irq)
{
    if ((unsigned)irq >= IRQ_TABLE_SIZE) return;
    const irq_desc_t *d = &irq_table[irq];
    uint8_t in_intcon = d->flag_in_intcon;
    uint8_t flag_mask = d->flag_mask;
    if (in_intcon)
    {
        EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), flag_mask);
    }
#if PIC14MIDRANGE_HAS_PIR1
    else
    {
        /* PIR1/PIR2 are Bank 0, so no pic_select_bank needed here. */
        uint8_t addr = pir_reg_addr(d);
        uint8_t v = EPIC_REG8(addr);
        v &= (uint8_t)~flag_mask;
        EPIC_REG8(addr) = v;
    }
#else
    else
    {
        /* PIR-less family (83/84/84A): the only non-INTCON flag is
         * EEIF, EECON1<4>, Bank 1: route through the literal-token
         * macros (same XC8 misdirect class as PIE1). */
        uint8_t v = 0U;
        (void)flag_mask;
        EPIC_BANK1_READ8(EECON1, v);
        v &= (uint8_t)~PIC_EECON1_EEIF;
        EPIC_BANK1_WRITE8(EECON1, v);
    }
#endif
}

/**
 * @brief Return the pending state of `irq` (1 = flag set).
 * @param irq the interrupt source to query.
 * @return 1 if the flag is set, 0 otherwise.
 */
uint8_t EPIC_IRQ_GetFlag(PIC16_IRQn irq)
{
    if ((unsigned)irq >= IRQ_TABLE_SIZE) return 0U;
    const irq_desc_t *d = &irq_table[irq];
    uint8_t in_intcon = d->flag_in_intcon;
    uint8_t flag_mask = d->flag_mask;
#if PIC14MIDRANGE_HAS_PIR1
    uint8_t addr = pir_reg_addr(d);
    uint8_t reg = in_intcon ? EPIC_REG8(PIC_REG_INTCON) : EPIC_REG8(addr);
    return (reg & flag_mask) ? 1U : 0U;
#else
    if (in_intcon)
    {
        return (EPIC_REG8(PIC_REG_INTCON) & flag_mask) ? 1U : 0U;
    }
    /* PIR-less family: EEIF is EECON1<4> (Bank 1, literal-token read). */
    uint8_t v = 0U;
    EPIC_BANK1_READ8(EECON1, v);
    return (v & PIC_EECON1_EEIF) ? 1U : 0U;
#endif
}

/**
 * @brief Set the priority of `irq`. No-op on PIC16 (single vector, no
 *        priority scheme); present for portability with PIC18.
 * @param irq the interrupt source (ignored on PIC16).
 * @param prio the requested priority (ignored on PIC16).
 */
void EPIC_IRQ_SetPriority(PIC16_IRQn irq, EPIC_IRQ_Priority prio)
{
    /* PIC14 mid-range has a single interrupt vector, no priority scheme
     * (DS39582B §14.11). This is the no-op half of the shared
     * EPIC_IRQ_SetPriority contract; PIC18's implementation writes the
     * matching IPR bit. Both arguments are intentionally unused. */
    (void)irq;
    (void)prio;
}
