/*
 * Timer0 driver, implementation (DS39632E §11.0, Register 11-1).
 */

#include "peripherals/pic18fxx5x_timer0.h"
#include "core/pic18_irq.h"

/* Prescaler ratios, DS39632E Table 11-1.
 *   T0PS 000 -> 1:2, 001 -> 1:4, ... 111 -> 1:256. */
static const uint16_t ps_ratio[8] = { 2, 4, 8, 16, 32, 64, 128, 256 };

/** Per-handle storage; `EPIC_TIMER0_Init` COPIES the caller's handle here
 *  (the caller's struct is typically stack-local, out of scope by the time
 *  the ISR reads it back, so storing a pointer would dangle). */
static TIMER0_HandleTypeDef g_t0_storage;
static const TIMER0_HandleTypeDef *g_t0_handle = NULL;

/**
 * @brief  Atomic read-modify-write of T0CON: clear `clr_mask` bits, then
 *         set `set_mask` bits.
 * @param clr_mask Bitmask of T0CON bits to clear.
 * @param set_mask Bitmask of T0CON bits to set.
 */
static void t0con_clr_set(uint8_t clr_mask, uint8_t set_mask)
{
    uint8_t t = EPIC_REG8(PIC_REG_T0CON);
    t = (uint8_t)((t & (uint8_t)~clr_mask) | set_mask);
    EPIC_REG8(PIC_REG_T0CON) = t;
}

/**
 * @brief  Initialize Timer0 from a handle: stop the timer, clear TMR0IF,
 *         enable the overflow interrupt if a callback is given, select
 *         the 8/16-bit mode and store an owned copy of the handle.
 * @param h Handle describing the Timer0 configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Init(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Stop the timer before reconfiguring (TMR0ON = 0). */
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_TMR0ON);

    /* Clear TMR0IF; configure TMR0IE if a callback is provided. */
    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR0);
    if (h->OverflowCallback)
    {
        EPIC_IRQ_Enable(PIC18_IRQ_TMR0);
    }
    else
    {
        EPIC_IRQ_DisableSrc(PIC18_IRQ_TMR0);
    }

    /* Set the 8/16-bit mode bit (T0CON<T08BIT>). */
    if (h->Mode == TIMER0_BITMODE_8BIT)
    {
        EPIC_BIT_SET(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_T08BIT);
    }
    else
    {
        EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_T08BIT);
    }

    g_t0_storage = *h;
    g_t0_handle = &g_t0_storage;
    return EPIC_OK;
}

/**
 * @brief  De-initialize Timer0: disable its interrupt, clear the flag,
 *         stop the timer, zero TMR0L/TMR0H.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_TIMER0_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC18_IRQ_TMR0);
    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR0);
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_TMR0ON);
    EPIC_REG8(PIC_REG_TMR0L) = 0x00U;
    EPIC_REG8(PIC_REG_TMR0H) = 0x00U;
    return EPIC_OK;
}

/**
 * @brief  Start Timer0: load the reload value, program the prescaler,
 *         clock source, edge and mode in one T0CON read-modify-write,
 *         then set TMR0ON.
 * @param h Handle whose configuration is applied (reload value,
 *           prescaler, clock source, edge, mode).
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Start(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Load the counter. In 8-bit mode only TMR0L is used; in 16-bit mode
     * TMR0H is the high byte (DS39632E §11.0). */
    EPIC_REG8(PIC_REG_TMR0L) = h->ReloadValue;
    if (h->Mode == TIMER0_BITMODE_16BIT)
    {
        EPIC_REG8(PIC_REG_TMR0H) = 0x00U;
    }

    /* Program prescaler assignment + ratio + clock source + edge + mode in
     * one atomic read-modify-write of T0CON. */
    uint8_t set_mask = (uint8_t)(h->Prescaler & PIC_T0CON_T0PS_MASK);
    if (!h->PrescalerAssigned) set_mask |= PIC_T0CON_PSA;
    if (h->ClockSource == TIMER0_CLOCK_EXTERNAL) set_mask |= PIC_T0CON_T0CS;
    if (h->ClockEdge   == TIMER0_EDGE_FALLING)   set_mask |= PIC_T0CON_T0SE;
    if (h->Mode == TIMER0_BITMODE_8BIT)          set_mask |= PIC_T0CON_T08BIT;

    uint8_t clr_mask = (uint8_t)(PIC_T0CON_T0PS_MASK | PIC_T0CON_PSA |
                                 PIC_T0CON_T0CS  | PIC_T0CON_T0SE |
                                 PIC_T0CON_T08BIT);
    t0con_clr_set(clr_mask, set_mask);

    /* Start the timer (TMR0ON = 1). */
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_TMR0ON);
    return EPIC_OK;
}

/**
 * @brief  Stop Timer0 by clearing TMR0ON.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Stop(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T0CON), PIC_T0CON_TMR0ON);
    return EPIC_OK;
}

/**
 * @brief  Read the Timer0 counter low byte (TMR0L). Reading TMR0L latches
 *         TMR0H on real hardware; the low byte is all this 8-bit API
 *         returns.
 * @return The current TMR0L value.
 */
uint8_t EPIC_TIMER0_ReadCounter(void)
{
    /* Reading TMR0L latches TMR0H on real hardware (DS39632E §11.0); the
     * low byte is all this 8-bit API returns. */
    return EPIC_REG8(PIC_REG_TMR0L);
}

/**
 * @brief  Write the Timer0 counter low byte (TMR0L).
 * @param value Byte to load into TMR0L.
 */
void EPIC_TIMER0_WriteCounter(uint8_t value)
{
    EPIC_REG8(PIC_REG_TMR0L) = value;
}

/**
 * @brief  Map a Timer0 prescaler enum to its ratio (DS39632E Table 11-1).
 * @param p Prescaler selection (T0PS<2:0> value).
 * @return The divide ratio (2..256), or 1 for an out-of-range value.
 */
uint16_t EPIC_TIMER0_PrescalerToRatio(TIMER0_PrescalerTypeDef p)
{
    if ((unsigned)p > 7U) return 1U;
    return ps_ratio[p];
}

/**
 * @brief  Weak Timer0 interrupt handler: clears TMR0IF and invokes the
 *         overflow callback registered via Init.
 */
void TIMER0_IRQHandler(void)
{
    if (!EPIC_IRQ_GetFlag(PIC18_IRQ_TMR0)) return;
    EPIC_IRQ_ClearFlag(PIC18_IRQ_TMR0);
    if (g_t0_handle && g_t0_handle->OverflowCallback)
    {
        g_t0_handle->OverflowCallback();
    }
}
