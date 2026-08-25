/* Vendored slice of the real epic-hal pic18fxx5x-hal Timer2 driver
 * (DS39632E §12.0), adapted to compile under epic-cc. Timer2 is the
 * auto-reload 1 ms timebase of epic-tick: PR2 period, prescaler, and a
 * postscaler, with TMR2IF firing on each PR2 match.
 *
 * Adapted around two open backend gaps (see hal_pic18.h): the real driver
 * copies the caller's handle into static storage (`g_t2_storage = *h`,
 * an i64 struct copy the epic-cc irparse rejects), so the epic-cc variant
 * stores the fields it needs instead; and the ISR invokes an
 * OverflowCallback through a function pointer (epic-cc#73), so it clears
 * TMR2IF and the smoke's poll loop advances the tick directly.
 */

#include "hal_pic18.h"

typedef enum {
    TIMER2_PRESCALER_1_1  = 0x0U,
    TIMER2_PRESCALER_1_4  = 0x1U,
    TIMER2_PRESCALER_1_16 = 0x2U
} TIMER2_PrescalerTypeDef;

typedef enum {
    TIMER2_POSTSCALER_1_1  = 0x0U,
    TIMER2_POSTSCALER_1_2  = 0x1U,
    TIMER2_POSTSCALER_1_3  = 0x2U,
    TIMER2_POSTSCALER_1_4  = 0x3U,
    TIMER2_POSTSCALER_1_5  = 0x4U,
    TIMER2_POSTSCALER_1_6  = 0x5U,
    TIMER2_POSTSCALER_1_7  = 0x6U,
    TIMER2_POSTSCALER_1_8  = 0x7U,
    TIMER2_POSTSCALER_1_9  = 0x8U,
    TIMER2_POSTSCALER_1_10 = 0x9U,
    TIMER2_POSTSCALER_1_11 = 0xAU,
    TIMER2_POSTSCALER_1_12 = 0xBU,
    TIMER2_POSTSCALER_1_13 = 0xCU,
    TIMER2_POSTSCALER_1_14 = 0xDU,
    TIMER2_POSTSCALER_1_15 = 0xEU,
    TIMER2_POSTSCALER_1_16 = 0xFU
} TIMER2_PostscalerTypeDef;

typedef struct {
    TIMER2_PrescalerTypeDef  Prescaler;
    TIMER2_PostscalerTypeDef Postscaler;
    uint8_t                  Period; /* PR2 value, 0..255 */
} TIMER2_HandleTypeDef;

void TIMER2_IRQHandler(void)
{
    if (!(EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR2IF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
}

EPIC_StatusTypeDef EPIC_TIMER2_Init(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_PIE1), PIC_PIE1_TMR2IE);
    return EPIC_OK;
}

EPIC_StatusTypeDef EPIC_TIMER2_Start(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    /* Period register first (DS39632E §12.0). */
    EPIC_REG8(PIC_REG_PR2) = h->Period;
    uint8_t v = (uint8_t)((h->Postscaler & 0xFU) << 3);
    v |= PIC_T2CON_TMR2ON;
    v |= (uint8_t)(h->Prescaler & 0x3U);
    EPIC_REG8(PIC_REG_T2CON) = v;
    return EPIC_OK;
}
