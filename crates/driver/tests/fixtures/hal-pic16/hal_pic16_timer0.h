/* Timer0 surface of the vendored hal-pic16 slice: handle, default, and
 * the inlined Init/Start. Inline in the header so the callback store
 * lands in the caller's TU and clang folds the handle-field load to a
 * named literal, which is what lets epic-cc resolve the cross-context
 * ISR dispatch (ADR-024, epic-hal#105). */

#ifndef HAL_PIC16_TIMER0_H
#define HAL_PIC16_TIMER0_H

#include "hal_pic16.h"
#include <stddef.h>

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
    TIMER0_ClockSourceTypeDef  ClockSource;
    TIMER0_ClockEdgeTypeDef    ClockEdge;
    TIMER0_PrescalerTypeDef    Prescaler;
    bool                       PrescalerAssigned;
    uint8_t                    ReloadValue;
    void (*OverflowCallback)(void);
} TIMER0_HandleTypeDef;

#define TIMER0_HANDLE_DEFAULT {                                         \
    .ClockSource        = TIMER0_CLOCK_INTERNAL,                        \
    .ClockEdge          = TIMER0_EDGE_RISING,                           \
    .Prescaler          = TIMER0_PRESCALER_1_256,                       \
    .PrescalerAssigned  = true,                                         \
    .ReloadValue        = 0x00U,                                        \
    .OverflowCallback   = NULL,                                         \
}

/* The ISR's owned callback slot, defined in hal_pic16_timer0.c. */
extern void (*g_t0_overflow_cb)(void);

/* IRQ controller entry points used by the inlined Init. */
void EPIC_IRQ_ClearFlag(void);
void EPIC_IRQ_Enable(void);
void EPIC_IRQ_DisableSrc(void);

/**
 * @brief Configure Timer0: stop it, arm the overflow interrupt if a
 *        callback is given, and record the callback. Static inline so
 *        the callback store lands in the caller's TU (ADR-024).
 * @param h handle with ClockSource, ClockEdge, Prescaler,
 *        PrescalerAssigned, ReloadValue, OverflowCallback.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
static inline EPIC_StatusTypeDef EPIC_TIMER0_Init(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* Stop the timer: clear T0CS (DS39582B §5.0). */
    uint8_t opt = EPIC_REG8(PIC_REG_OPTION);
    opt = (uint8_t)(opt & (uint8_t)~PIC_OPTION_T0CS);
    EPIC_REG8(PIC_REG_OPTION) = opt;

    EPIC_IRQ_ClearFlag();
    if (h->OverflowCallback) {
        EPIC_IRQ_Enable();
    } else {
        EPIC_IRQ_DisableSrc();
    }

    g_t0_overflow_cb = h->OverflowCallback;
    return EPIC_OK;
}

/**
 * @brief Start Timer0 counting: reload TMR0 and program the prescaler
 *        assignment/ratio, clock source and edge. Static inline for the
 *        same reason as Init: with the handle local to the caller, clang
 *        folds every field load.
 * @param h handle whose ReloadValue and config are applied.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
static inline EPIC_StatusTypeDef EPIC_TIMER0_Start(const TIMER0_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;

    /* DS39582B §5.3: writing TMR0 when the prescaler is assigned to
     * Timer0 clears the prescaler. */
    EPIC_REG8(PIC_REG_TMR0) = h->ReloadValue;

    uint8_t set_mask = (uint8_t)((h->Prescaler & PIC_OPTION_PS_MASK));
    if (!h->PrescalerAssigned) set_mask |= PIC_OPTION_PSA;
    if (h->ClockSource == TIMER0_CLOCK_EXTERNAL) set_mask |= PIC_OPTION_T0CS;
    if (h->ClockEdge   == TIMER0_EDGE_FALLING)  set_mask |= PIC_OPTION_T0SE;

    /* Mask leaves RBPU and INTEDG untouched (DS39582B §4.2 / §14.12.4). */
    uint8_t clr_mask = (uint8_t)(PIC_OPTION_PS_MASK | PIC_OPTION_PSA |
                                 PIC_OPTION_T0CS  | PIC_OPTION_T0SE);
    uint8_t opt = EPIC_REG8(PIC_REG_OPTION);
    opt = (uint8_t)((opt & (uint8_t)~clr_mask) | set_mask);
    EPIC_REG8(PIC_REG_OPTION) = opt;

    return EPIC_OK;
}

#endif /* HAL_PIC16_TIMER0_H */
