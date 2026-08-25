/* Vendored slice of the real epic-hal pic18fxx5x-hal Timer0 driver
 * (DS39632E §11.0), adapted to compile under epic-cc. Reduced to the
 * blink surface: 8-bit mode, internal Fosc/4, 1:256 prescaler, reload 0.
 * The ISR clears TMR0IF; the overflow callback is not invoked under
 * epic-cc (calls through a function pointer are an open backend gap,
 * epic-cc#73), so the smoke's main loop polls TMR0IF directly, exactly
 * as the 887 HAL's epic-cc timer0 variant does.
 */

#include "hal_pic18.h"

/* T0CON bits (DS39632E Register 11-1). */
#define PIC_T0CON_T0PS_MASK 0x07U
#define PIC_T0CON_T0PSA      EPIC_BIT(3)
#define PIC_T0CON_T0SE       EPIC_BIT(4)
#define PIC_T0CON_T0CS       EPIC_BIT(5)
#define PIC_T0CON_T08BIT     EPIC_BIT(6)
#define PIC_T0CON_TMR0ON     EPIC_BIT(7)

typedef enum {
    TIMER0_BITMODE_16BIT = 0x0U,
    TIMER0_BITMODE_8BIT  = 0x1U
} TIMER0_BitModeTypeDef;

typedef enum {
    TIMER0_CLOCK_INTERNAL = 0x0U,
    TIMER0_CLOCK_EXTERNAL = 0x1U
} TIMER0_ClockSourceTypeDef;

typedef enum {
    TIMER0_EDGE_RISING  = 0x0U,
    TIMER0_EDGE_FALLING = 0x1U
} TIMER0_ClockEdgeTypeDef;

typedef enum {
    TIMER0_PRESCALER_1_2    = 0x0U,
    TIMER0_PRESCALER_1_4    = 0x1U,
    TIMER0_PRESCALER_1_8    = 0x2U,
    TIMER0_PRESCALER_1_16   = 0x3U,
    TIMER0_PRESCALER_1_32   = 0x4U,
    TIMER0_PRESCALER_1_64   = 0x5U,
    TIMER0_PRESCALER_1_128  = 0x6U,
    TIMER0_PRESCALER_1_256  = 0x7U
} TIMER0_PrescalerTypeDef;

typedef struct {
    TIMER0_BitModeTypeDef     Mode;
    TIMER0_ClockSourceTypeDef ClockSource;
    TIMER0_ClockEdgeTypeDef   ClockEdge;
    TIMER0_PrescalerTypeDef   Prescaler;
    uint8_t                   PrescalerAssigned;
    uint8_t                   ReloadValue;
} TIMER0_HandleTypeDef;

void TIMER0_IRQHandler(void)
{
    if (!(EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
}

EPIC_StatusTypeDef EPIC_TIMER0_Init(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Stop the timer before reconfiguring. */
    EPIC_BIT_CLR(EPIC_REG8(0xFD5U), PIC_T0CON_TMR0ON); /* T0CON, stop the timer */

    /* Clear the overflow flag; arm TMR0IE only if a callback was given.
     * Under epic-cc the smoke polls the flag, so the source enable is
     * left on. */
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IE);
    return EPIC_OK;
}

EPIC_StatusTypeDef EPIC_TIMER0_Start(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Reload TMR0L. */
    EPIC_REG8(0xFD6U) = h->ReloadValue;

    /* Program T0CON: 8-bit mode, internal clock, rising edge, prescaler
     * 1:256 assigned to Timer0, then TMR0ON. */
    uint8_t v = PIC_T0CON_T08BIT | PIC_T0CON_TMR0ON;
    if (h->ClockSource == TIMER0_CLOCK_EXTERNAL) v |= PIC_T0CON_T0CS;
    if (h->ClockEdge   == TIMER0_EDGE_FALLING)  v |= PIC_T0CON_T0SE;
    if (h->PrescalerAssigned == 0u)              v |= PIC_T0CON_T0PSA;
    v |= (uint8_t)(h->Prescaler & PIC_T0CON_T0PS_MASK);
    EPIC_REG8(0xFD5U) = v; /* T0CON */
    return EPIC_OK;
}
