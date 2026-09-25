/*
 * Timer2 driver, implementation (DS39632E §12.0, Register 12-2).
 */

#include "peripherals/pic18fxx5x_timer2.h"
#include "core/pic18_irq.h"

/* T2CON prescaler, DS39632E Register 12-2:
 *   00 -> 1:1, 01 -> 1:4, 1x -> 1:16 */
static const uint8_t pre_ratio[4] = { 1, 4, 16, 16 };

/* T2CON postscaler, DS39632E Register 12-2: 1:(N+1). */
static const uint8_t post_ratio[16] = {
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16
};

/** Per-handle storage. COPIES the caller's handle (see Timer1 for the
 *  dangling-pointer rationale). The weak ISR reads from this owned copy. */
static TIMER2_HandleTypeDef g_t2_storage;
static const TIMER2_HandleTypeDef *g_t2_handle = NULL;

/**
 * @brief  Read the Timer2 counter register (TMR2).
 * @return The current TMR2 value.
 */
uint8_t EPIC_TIMER2_ReadCounter(void)
{
    return EPIC_REG8(PIC_REG_TMR2);
}

/**
 * @brief  Write the Timer2 counter register (TMR2).
 * @param value Byte to load into TMR2.
 */
void EPIC_TIMER2_WriteCounter(uint8_t value)
{
    EPIC_REG8(PIC_REG_TMR2) = value;
}

/**
 * @brief  Read the Timer2 period register (PR2).
 * @return The current PR2 value.
 */
uint8_t EPIC_TIMER2_ReadPeriod(void)
{
    /* PR2 is in the Access Bank (0xFCB), no bank switching needed. */
    return EPIC_REG8(PIC_REG_PR2);
}

/**
 * @brief  Write the Timer2 period register (PR2).
 * @param period Byte to load into PR2.
 */
void EPIC_TIMER2_WritePeriod(uint8_t period)
{
    EPIC_REG8(PIC_REG_PR2) = period;
}

/**
 * @brief  Map a Timer2 prescaler enum to its ratio (DS39632E Register
 *         12-2: 00 -> 1:1, 01 -> 1:4, 1x -> 1:16).
 * @param p Prescaler selection (T2CKPS<1:0> value).
 * @return The divide ratio (1..16), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER2_PrescalerToRatio(TIMER2_PrescalerTypeDef p)
{
    if ((unsigned)p > 3U) return 1U;
    return pre_ratio[p];
}

/**
 * @brief  Map a Timer2 postscaler enum to its ratio (DS39632E Register
 *         12-2: 1:(N+1)).
 * @param p Postscaler selection (T2OUTPS<3:0> value).
 * @return The divide ratio (1..16), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER2_PostscalerToRatio(TIMER2_PostscalerTypeDef p)
{
    if ((unsigned)p > 15U) return 1U;
    return post_ratio[p];
}

/**
 * @brief  Initialize Timer2 from a handle: stop the timer, clear TMR2IF,
 *         enable the overflow interrupt if a callback is given and store
 *         an owned copy of the handle.
 * @param h Handle describing the Timer2 configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER2_Init(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);

    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR2);
    if (h->OverflowCallback)
    {
        EPIC_IRQ_Enable(PIC18_IRQ_TMR2);
    }
    else
    {
        EPIC_IRQ_DisableSrc(PIC18_IRQ_TMR2);
    }

    g_t2_storage = *h;
    g_t2_handle = &g_t2_storage;
    return EPIC_OK;
}

/**
 * @brief  De-initialize Timer2: disable its interrupt, clear the flag,
 *         restore T2CON to its power-on value, reset PR2 and drop the
 *         stored handle.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_TIMER2_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC18_IRQ_TMR2);
    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR2);
    EPIC_REG8(PIC_REG_T2CON) = PIC_T2CON_POR_VALUE;
    EPIC_TIMER2_WritePeriod(PIC_PR2_POR_VALUE);
    g_t2_handle = NULL;
    return EPIC_OK;
}

/**
 * @brief  Start Timer2: write the period register first (to avoid
 *         spurious matches) then program T2CON (postscaler, prescaler)
 *         and set TMR2ON.
 * @param h Handle whose configuration is applied.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER2_Start(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Period register first (DS39632E §12.0 recommends setting PR2 before
     * enabling TMR2ON to avoid spurious matches). */
    EPIC_TIMER2_WritePeriod(h->Period);

    /* Build T2CON:
     *   T2OUTPS3:T2OUTPS0 -> bits 6:3
     *   TMR2ON            -> bit 2
     *   T2CKPS1:T2CKPS0   -> bits 1:0
     */
    uint8_t v = 0U;
    v |= (uint8_t)((h->Postscaler & 0xFU) << 3);
    v |= PIC_T2CON_TMR2ON;
    v |= (uint8_t)(h->Prescaler & 0x3U);
    EPIC_REG8(PIC_REG_T2CON) = v;

    return EPIC_OK;
}

/**
 * @brief  Stop Timer2 by clearing TMR2ON.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_TIMER2_Stop(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);
    return EPIC_OK;
}

/**
 * @brief  Weak Timer2 interrupt handler: clears TMR2IF and invokes the
 *         overflow callback registered via Init.
 */
void TIMER2_IRQHandler(void)
{
    if (!EPIC_IRQ_GetFlag(PIC18_IRQ_TMR2)) return;
    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR2);
    if (g_t2_handle && g_t2_handle->OverflowCallback)
    {
        g_t2_handle->OverflowCallback();
    }
}
