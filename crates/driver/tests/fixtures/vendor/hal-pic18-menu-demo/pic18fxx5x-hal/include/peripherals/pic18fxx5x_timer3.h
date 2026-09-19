/*
 * Timer3 driver, 16-bit timer/counter (DS39632E §14.0), a second
 * 16-bit timer alongside Timer1. Shares Timer1's T1OSC (no T3OSCEN of
 * its own); overflow sets PIR2<TMR3IF>. T3CON<T3CCP2:T3CCP1> selects
 * Timer1 vs Timer3 as the CCP capture/compare source, left at reset
 * (Timer1) here; the CCP/ECCP driver manages it when needed.
 */

#ifndef PIC18FXX5X_TIMER3_H
#define PIC18FXX5X_TIMER3_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief Timer3 clock source (T3CON<TMR3CS>, DS39632E Register 14-1).
 */
typedef enum {
    TIMER3_CLOCK_INTERNAL  = 0x0U,   /**< Fosc/4 (timer mode). */
    TIMER3_CLOCK_EXTERNAL  = 0x1U,   /**< External pin or T1OSC (shared w/ Timer1). */
} TIMER3_ClockSourceTypeDef;

/**
 * @brief External-clock synchronisation (T3CON<T3SYNC>, DS39632E §14.0).
 *        Ignored in timer mode (internal clock).
 */
typedef enum {
    TIMER3_SYNC_EXTERNAL   = 0x0U,   /**< T3SYNC = 0, synchronise to Fosc. */
    TIMER3_ASYNC_EXTERNAL  = 0x1U,   /**< T3SYNC = 1, free-running. */
} TIMER3_ClockSyncTypeDef;

/**
 * @brief Prescaler ratio (T3CON<T3CKPS1:T3CKPS0>, DS39632E Register 14-1).
 */
typedef enum {
    TIMER3_PRESCALER_1_1 = 0x0U,    /**< 1:1, 00. */
    TIMER3_PRESCALER_1_2 = 0x1U,    /**< 1:2, 01. */
    TIMER3_PRESCALER_1_4 = 0x2U,    /**< 1:4, 10. */
    TIMER3_PRESCALER_1_8 = 0x3U,    /**< 1:8, 11. */
} TIMER3_PrescalerTypeDef;

/** Driver handle (Cube-style). No oscillator field: Timer3 shares Timer1's
 *  T1OSC (enable it via the Timer1 driver if needed). */
typedef struct {
    TIMER3_ClockSourceTypeDef  ClockSource;
    TIMER3_ClockSyncTypeDef    ClockSync;
    TIMER3_PrescalerTypeDef    Prescaler;
    uint16_t                   ReloadValue;   /**< 16-bit initial counter. */
    /** @brief Optional overflow callback (fires on PIR2<TMR3IF>). */
    void (*OverflowCallback)(void);
} TIMER3_HandleTypeDef;

#define TIMER3_HANDLE_DEFAULT {                                         \
    .ClockSource      = TIMER3_CLOCK_INTERNAL,                          \
    .ClockSync        = TIMER3_SYNC_EXTERNAL,                           \
    .Prescaler        = TIMER3_PRESCALER_1_1,                            \
    .ReloadValue      = 0x0000U,                                        \
    .OverflowCallback = NULL,                                            \
}

/**
 * @brief  Configure Timer3 from the handle: clock source, sync, prescaler
 *         and reload value, then set RD16 for atomic 16-bit access. Does
 *         not start the timer, call @ref EPIC_TIMER3_Start.
 * @param h the Timer3 handle describing the desired configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER3_Init(const TIMER3_HandleTypeDef *h);

/**
 * @brief  Disable Timer3 counting and clear TMR3IF.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER3_DeInit(void);

/**
 * @brief  Enable Timer3 counting: writes the reload value then sets
 *         T3CON<TMR3ON>.
 * @param h the handle used to configure the timer.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER3_Start(const TIMER3_HandleTypeDef *h);

/**
 * @brief  Disable Timer3 counting. Clears T3CON<TMR3ON>.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER3_Stop(void);

/**
 * @brief Atomically read the 16-bit counter (RD16 latches TMR3H on TMR3L read).
 * @return the current 16-bit TMR3 value.
 */
uint16_t EPIC_TIMER3_ReadCounter(void);

/**
 * @brief Atomically write the 16-bit counter (RD16 latches TMR3H on TMR3L write).
 * @param value the 16-bit value to load into the counter.
 */
void EPIC_TIMER3_WriteCounter(uint16_t value);

/**
 * @brief Convert a prescaler enum to its integer ratio (1, 2, 4, 8).
 * @param p the prescaler enum value.
 * @return the integer division ratio (1, 2, 4 or 8).
 */
uint16_t EPIC_TIMER3_PrescalerToRatio(TIMER3_PrescalerTypeDef p);

/**
 * @brief Weak Timer3 ISR, override in user code to add application logic.
 */
void TIMER3_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_TIMER3_H */
