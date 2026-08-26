/* Vendored slice of the real epic-hal pic16f87xa-hal IRQ controller
 * (DS39582B §14.11), adapted to compile under epic-cc. The const
 * `irq_table` is the regression shape of epic-cc#114: a static const
 * array of a named struct, read by field, must emit its real
 * initializer bytes (never a zero blob). The e2e reads a field through
 * EPIC_IRQ_GetFlagMask's runtime-indexed path and asserts non-zero.
 */

#include "hal_pic16.h"

/* Per-IRQ descriptor: which register the enable / flag bit lives in. */
typedef struct {
    uint8_t flag_mask;     /**< PIR/INTCON bit to test/clear. */
    uint8_t enable_mask;   /**< PIE/INTCON bit to set/clear. */
    uint8_t in_intcon;     /**< 1 = INTCON, 0 = PIR1/PIR2. */
    uint8_t pir_is_pir2;   /**< 1 = PIR2, 0 = PIR1. (Ignored if in_intcon.) */
} irq_desc_t;

static const irq_desc_t irq_table[] = {
    [2] = { PIC_INTCON_TMR0IF, PIC_INTCON_TMR0IE, 1, 0 },
    [0] = { PIC_INTCON_RBIF,   PIC_INTCON_RBIE,   1, 0 },
};

#define IRQ_TABLE_SIZE  (sizeof irq_table / sizeof irq_table[0])

/**
 * @brief Clear the Timer0 interrupt flag (INTCON<TMR0IF>, Bank 0).
 */
void EPIC_IRQ_ClearFlag(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
}

/**
 * @brief Enable the Timer0 overflow interrupt source (INTCON<TMR0IE>).
 */
void EPIC_IRQ_Enable(void)
{
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IE);
}

/**
 * @brief Disable the Timer0 overflow interrupt source.
 */
void EPIC_IRQ_DisableSrc(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IE);
}

/**
 * @brief Restore the global interrupt enable (INTCON<GIE>).
 * @param prev_state 1 to enable interrupts globally.
 */
void EPIC_IRQ_Restore(uint8_t prev_state)
{
    if (prev_state) {
        EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_GIE);
    } else {
        EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_GIE);
    }
}

/**
 * @brief Read a const `irq_table` descriptor field through a RUNTIME
 *        index (the epic-cc#114 regression shape: a const table read by
 *        field must emit its real initializer bytes, never a zero blob).
 * @param irq the interrupt source index.
 * @return the descriptor's flag bit mask (non-zero for every entry).
 */
uint8_t EPIC_IRQ_GetFlagMask(uint8_t irq)
{
    if ((unsigned)irq >= IRQ_TABLE_SIZE) return 0U;
    return irq_table[irq].flag_mask;
}
