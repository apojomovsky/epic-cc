/* Timer2 driver, 8-bit timer with PR2 period register and postscaler.
 * Source: DS39582B §7.0; full reference: MANUAL.md §12. TMR2 resets to
 * 0 on TMR2==PR2 (never reaches PR2+1); TMR2IF fires every prescaler x
 * postscaler x (PR2+1) cycles and drives the CCP PWM time base. */

#ifndef PIC16F87XA_TIMER2_H
#define PIC16F87XA_TIMER2_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"
#include "core/pic16_irq.h"

/**
 * @brief Prescaler ratio (T2CON<T2CKPS1:T2CKPS0>, DS39582B §7.0, Reg 7-1).
 */
typedef enum {
    TIMER2_PRESCALER_1_1  = 0x0U,    /**< 00. */
    TIMER2_PRESCALER_1_4  = 0x1U,    /**< 01. */
    TIMER2_PRESCALER_1_16 = 0x2U,    /**< 1x. */
} TIMER2_PrescalerTypeDef;

/**
 * @brief Postscaler ratio (T2CON<TOUTPS3:TOUTPS0>, DS39582B §7.0).
 *        Postscaler value N → 1:(N+1) divider.
 */
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
    TIMER2_POSTSCALER_1_16 = 0xFU,
} TIMER2_PostscalerTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    TIMER2_PrescalerTypeDef    Prescaler;
    TIMER2_PostscalerTypeDef   Postscaler;
    uint8_t                    Period;       /**< PR2 value, 0..255. */
    /** @brief Optional overflow callback (fires on TMR2IF, i.e. every
     *         prescaler × (PR2+1) × postscaler cycles). */
    void (*OverflowCallback)(void);
} TIMER2_HandleTypeDef;

#define TIMER2_HANDLE_DEFAULT {                                         \
    .Prescaler       = TIMER2_PRESCALER_1_1,                            \
    .Postscaler      = TIMER2_POSTSCALER_1_1,                           \
    .Period          = 0xFFU,                                           \
    .OverflowCallback = NULL,                                           \
}

/* The ISR's owned callback slot, defined in the driver body. */
extern void (*g_t2_overflow_cb)(void);

/** @brief Write the PR2 period register (Bank 1); forward-declared so
 *         the inlined Start can call it before its own declaration.
 * @param period the 8-bit PR2 value, 0..255. */
void EPIC_TIMER2_WritePeriod(uint8_t period);

/**
 * @brief  Initialize Timer2 from the handle. Static inline so the
 *         callback store lands in the caller's TU as a named literal,
 *         which the epic-cc cross-context analysis resolves (ADR-024).
 * @param h handle with Prescaler, Postscaler, Period, OverflowCallback.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
static inline EPIC_StatusTypeDef EPIC_TIMER2_Init(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);
    EPIC_IRQ_ClearFlag(PIC16_IRQ_TMR2);
    if (h->OverflowCallback) {
        EPIC_IRQ_Enable(PIC16_IRQ_TMR2);
    } else {
        EPIC_IRQ_DisableSrc(PIC16_IRQ_TMR2);
    }
    g_t2_overflow_cb = h->OverflowCallback;
    return EPIC_OK;
}

/**
 * @brief  De-initialize Timer2. Disables the overflow interrupt and
 *         returns T2CON to reset.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER2_DeInit(void);

/**
 * @brief  Start Timer2 counting. Static inline for the same reason as Init.
 * @param h handle whose Period is loaded into PR2.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
static inline EPIC_StatusTypeDef EPIC_TIMER2_Start(const TIMER2_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    EPIC_TIMER2_WritePeriod(h->Period);
    uint8_t v = 0U;
    v |= (uint8_t)((h->Postscaler & 0xFU) << 3);
    v |= PIC_T2CON_TMR2ON;
    v |= (uint8_t)(h->Prescaler & 0x3U);
    EPIC_REG8(PIC_REG_T2CON) = v;
    return EPIC_OK;
}

/**
 * @brief  Stop Timer2 counting. Clears TMR2ON.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER2_Stop(void);

/**
 * @brief Read the current counter value.
 * @return the current 8-bit TMR2 value.
 */
uint8_t  EPIC_TIMER2_ReadCounter(void);

/**
 * @brief Write the counter value.
 * @param value the 8-bit value to load into TMR2.
 */
void     EPIC_TIMER2_WriteCounter(uint8_t value);

/**
 * @brief Read the period register value.
 * @return the current PR2 value.
 */
uint8_t  EPIC_TIMER2_ReadPeriod(void);

/**
 * @brief Write the period register value.
 * @param period the 8-bit PR2 value, 0..255.
 */
void     EPIC_TIMER2_WritePeriod(uint8_t period);

/**
 * @brief Convert a prescaler enum to its integer ratio (1, 4, 16).
 * @param p the prescaler enum value.
 * @return the integer prescaler ratio (1, 4 or 16).
 */
uint16_t EPIC_TIMER2_PrescalerToRatio(TIMER2_PrescalerTypeDef p);

/**
 * @brief Convert a postscaler enum to its integer ratio (1..16).
 * @param p the postscaler enum value.
 * @return the integer postscaler ratio (1..16).
 */
uint16_t EPIC_TIMER2_PostscalerToRatio(TIMER2_PostscalerTypeDef p);

/**
 * @brief Weak Timer2 ISR, override in user code to add application logic.
 */
void TIMER2_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_TIMER2_H */
