/* Timer0 driver implementation (DS39582B §5.0). */

#include "peripherals/pic16f87xa_timer0.h"
#include "core/pic16_irq.h"

/* Prescaler ratios, DS39582B Table 5-1: 000=1:2 ... 111=1:256. */
static const uint16_t ps_ratio[8] = { 2, 4, 8, 16, 32, 64, 128, 256 };

/* Owned copy of the caller's callback for the weak ISR (the caller's
 * handle is typically stack-local, out of scope by the time the ISR
 * reads it). The ISR only needs the callback, so store the pointer
 * (1 byte) rather than a full handle copy, the same RAM saving the
 * 88X driver makes. External linkage so the inlined Init in the
 * header can store to it from any TU. */
void (*g_t0_overflow_cb)(void) = NULL;

/**
 * @brief Read-modify-write helper for OPTION_REG: clear `clr_mask`
 *        bits, set `set_mask` bits, atomically.
 * @param clr_mask bits to clear.
 * @param set_mask bits to set.
 */
static void option_clr_set(uint8_t clr_mask, uint8_t set_mask)
{
#ifdef EPIC_BANK1_READ8
    /* Plain EPIC_REG8 RMW on Bank-1 OPTION_REG (0x81) misdirects to the
     * Bank-0 alias (0x01, TMR0) under XC8 v4.00 (see
     * target/pic16f87xa_platform.h). */
    uint8_t opt = 0u;
    EPIC_BANK1_READ8(OPTION_REG, opt);
    opt = (uint8_t)((opt & (uint8_t)~clr_mask) | set_mask);
    EPIC_BANK1_WRITE8(OPTION_REG, opt);
#else
    uint8_t opt = EPIC_REG8(PIC_REG_OPTION);
    opt = (uint8_t)((opt & (uint8_t)~clr_mask) | set_mask);
    EPIC_REG8(PIC_REG_OPTION) = opt;
#endif
}

/**
 * @brief De-initialize Timer0: disable the interrupt, stop counting
 *        and reset TMR0.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER0_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC16_IRQ_TMR0);
    EPIC_IRQ_ClearFlag(PIC16_IRQ_TMR0);
    option_clr_set(PIC_OPTION_T0CS, 0u);
    EPIC_REG8(PIC_REG_TMR0) = 0x00U;
    return EPIC_OK;
}

/**
 * @brief Stop Timer0 counting by clearing T0CS.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Stop(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_OPTION), PIC_OPTION_T0CS);
    return EPIC_OK;
}

/**
 * @brief Read the current TMR0 value.
 * @return the 8-bit counter value.
 */
uint8_t EPIC_TIMER0_ReadCounter(void)
{
    return EPIC_REG8(PIC_REG_TMR0);
}

/**
 * @brief Write the TMR0 counter (also clears the prescaler).
 * @param value the 8-bit value to load.
 */
void EPIC_TIMER0_WriteCounter(uint8_t value)
{
    EPIC_REG8(PIC_REG_TMR0) = value;
}

/**
 * @brief Convert a prescaler enum to its integer ratio.
 * @param p the prescaler enum value.
 * @return the ratio (2..256), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER0_PrescalerToRatio(TIMER0_PrescalerTypeDef p)
{
    if ((unsigned)p > 7U) return 1U;
    return ps_ratio[p];
}

/**
 * @brief Weak Timer0 ISR: clears TMR0IF and fires the overflow
 *        callback.
 */
void TIMER0_IRQHandler(void)
{
    /* Direct flag ops, not the table-driven EPIC_IRQ_* path: the retlw
     * table clobbers PCLATH in ISR context when on another page
     * (class-F hazard; see the CCP handlers). TMR0IF is INTCON bit 2. */
    if (!(EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
    if (g_t0_overflow_cb) g_t0_overflow_cb();
}
