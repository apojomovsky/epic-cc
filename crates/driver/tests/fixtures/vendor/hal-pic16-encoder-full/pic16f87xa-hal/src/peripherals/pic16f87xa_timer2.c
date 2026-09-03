/* Timer2 driver implementation (DS39582B §7.0). */

#include "peripherals/pic16f87xa_timer2.h"
#include "core/pic16_irq.h"

/* T2CON prescaler, DS39582B Register 7-1:
 *   00 → 1:1, 01 → 1:4, 1x → 1:16 */
static const uint8_t pre_ratio[4] = { 1, 4, 16, 16 };

/* T2CON postscaler, DS39582B Register 7-1: 1:(N+1). */
static const uint8_t post_ratio[16] = {
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16
};

void (*g_t2_overflow_cb)(void) = NULL;

/**
 * @brief Read the current TMR2 value.
 * @return the 8-bit counter value.
 */
uint8_t EPIC_TIMER2_ReadCounter(void)
{
    return EPIC_REG8(PIC_REG_TMR2);
}

/**
 * @brief Write the TMR2 counter.
 * @param value the 8-bit value to load.
 */
void EPIC_TIMER2_WriteCounter(uint8_t value)
{
    EPIC_REG8(PIC_REG_TMR2) = value;
}

/**
 * @brief Read the PR2 period register (Bank 1).
 * @return the current PR2 value.
 */
uint8_t EPIC_TIMER2_ReadPeriod(void)
{
#ifdef EPIC_BANK1_READ8
    /* Plain bank-switch read misdirects to the Bank-0 alias under XC8
     * v4.00 (see target/pic16f87xa_platform.h). */
    uint8_t pr2 = 0u;
    EPIC_BANK1_READ8(PR2, pr2);
    return pr2;
#else
    /* PR2 lives in Bank 1 (DS39582B Register 7-2, address 0x92). */
    uint8_t prev = (EPIC_REG8(PIC_REG_STATUS) >> 5) & 0x03U;
    pic_select_bank(1);
    uint8_t pr2 = EPIC_REG8(PIC_REG_PR2);
    pic_select_bank(prev);
    return pr2;
#endif
}

/**
 * @brief Write the PR2 period register (Bank 1).
 * @param period the 8-bit PR2 value, 0..255.
 */
void EPIC_TIMER2_WritePeriod(uint8_t period)
{
#ifdef EPIC_BANK1_WRITE8
    /* See target/pic16f87xa_platform.h: a plain bank-switch write here
     * silently corrupts under XC8 v4.00. */
    EPIC_BANK1_WRITE8(PR2, period);
#else
    uint8_t prev = (EPIC_REG8(PIC_REG_STATUS) >> 5) & 0x03U;
    pic_select_bank(1);
    EPIC_REG8(PIC_REG_PR2) = period;
    pic_select_bank(prev);
#endif
}

/**
 * @brief Convert a prescaler enum to its integer ratio.
 * @param p the prescaler enum value.
 * @return the ratio (1, 4 or 16), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER2_PrescalerToRatio(TIMER2_PrescalerTypeDef p)
{
    if ((unsigned)p > 3U) return 1U;
    return pre_ratio[p];
}

/**
 * @brief Convert a postscaler enum to its integer ratio.
 * @param p the postscaler enum value.
 * @return the ratio (1..16), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER2_PostscalerToRatio(TIMER2_PostscalerTypeDef p)
{
    if ((unsigned)p > 15U) return 1U;
    return post_ratio[p];
}

/**
 * @brief De-initialize Timer2: disable the interrupt and restore T2CON
 *        and PR2 to reset values.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER2_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC16_IRQ_TMR2);
    EPIC_IRQ_ClearFlag(PIC16_IRQ_TMR2);
    EPIC_REG8(PIC_REG_T2CON) = PIC_T2CON_POR_VALUE;
    EPIC_TIMER2_WritePeriod(0xFFU);
    g_t2_overflow_cb = NULL;
    return EPIC_OK;
}

/**
 * @brief Stop Timer2 counting by clearing TMR2ON.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER2_Stop(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);
    return EPIC_OK;
}

/**
 * @brief Weak Timer2 ISR: clears TMR2IF and fires the overflow
 *        callback.
 */
void TIMER2_IRQHandler(void)
{
    /* Direct flag ops (class-F: the table route clobbers PCLATH in ISR
     * context; see the CCP handlers). TMR2IF is PIR1 bit 1. */
    if (!(EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR2IF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
    if (g_t2_overflow_cb) {
        g_t2_overflow_cb();
    }
}
