/*
 * Timer1 driver, 16-bit timer/counter (DS39632E §12.0). PIC18's T1CON
 * adds RD16 (T1CON<7>, 16-bit read/write mode, set by this driver so the
 * atomic TMR1L-latches-TMR1H idiom works) and read-only T1RUN (ignored);
 * the other bits sit at the same positions as PIC16.
 */

#ifndef PIC18FXX5X_TIMER1_H
#define PIC18FXX5X_TIMER1_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief Timer1 clock source (T1CON<TMR1CS>, DS39632E Register 12-1).
 */
typedef enum {
    TIMER1_CLOCK_INTERNAL  = 0x0U,   /**< Fosc/4 (timer mode). */
    TIMER1_CLOCK_EXTERNAL  = 0x1U,   /**< External pin or T1OSC. */
} TIMER1_ClockSourceTypeDef;

/**
 * @brief External-clock synchronisation (T1CON<T1SYNC>, DS39632E §12.0).
 *        In timer mode (internal clock) this bit is ignored.
 */
typedef enum {
    TIMER1_SYNC_EXTERNAL   = 0x0U,   /**< T1SYNC = 0, synchronise to Fosc. */
    TIMER1_ASYNC_EXTERNAL  = 0x1U,   /**< T1SYNC = 1, free-running. */
} TIMER1_ClockSyncTypeDef;

/**
 * @brief T1OSC crystal-oscillator enable (T1CON<T1OSCEN>, DS39632E §12.0).
 *        Requires a 32.768 kHz crystal on T1OSI/T1OSO; the standard RTC
 *        time base.
 */
typedef enum {
    TIMER1_OSCILLATOR_OFF  = 0x0U,
    TIMER1_OSCILLATOR_ON   = 0x1U,
} TIMER1_OscillatorTypeDef;

/**
 * @brief Prescaler ratio (T1CON<T1CKPS1:T1CKPS0>, DS39632E Register 12-1).
 */
typedef enum {
    TIMER1_PRESCALER_1_1 = 0x0U,    /**< 1:1, 00. */
    TIMER1_PRESCALER_1_2 = 0x1U,    /**< 1:2, 01. */
    TIMER1_PRESCALER_1_4 = 0x2U,    /**< 1:4, 10. */
    TIMER1_PRESCALER_1_8 = 0x3U,    /**< 1:8, 11. */
} TIMER1_PrescalerTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    TIMER1_ClockSourceTypeDef  ClockSource;
    TIMER1_ClockSyncTypeDef    ClockSync;
    TIMER1_OscillatorTypeDef   Oscillator;
    TIMER1_PrescalerTypeDef    Prescaler;
    uint16_t                   ReloadValue;   /**< 16-bit initial counter. */
    /** @brief Optional overflow callback (fires on TMR1IF). */
    void (*OverflowCallback)(void);
} TIMER1_HandleTypeDef;

#define TIMER1_HANDLE_DEFAULT {                                         \
    .ClockSource      = TIMER1_CLOCK_INTERNAL,                          \
    .ClockSync        = TIMER1_SYNC_EXTERNAL,                           \
    .Oscillator       = TIMER1_OSCILLATOR_OFF,                          \
    .Prescaler        = TIMER1_PRESCALER_1_1,                            \
    .ReloadValue      = 0x0000U,                                        \
    .OverflowCallback = NULL,                                           \
}

/**
 * @brief  Configure Timer1 from the handle: clock source, sync, oscillator,
 *         prescaler and reload value, then set RD16 for atomic 16-bit
 *         access. Does not start the timer, call @ref EPIC_TIMER1_Start.
 * @param h the Timer1 handle describing the desired configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Init(const TIMER1_HandleTypeDef *h);

/**
 * @brief  Disable Timer1 counting and clear TMR1IF.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER1_DeInit(void);

/**
 * @brief  Enable Timer1 counting: writes the reload value then sets
 *         T1CON<TMR1ON>.
 * @param h the handle used to configure the timer.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Start(const TIMER1_HandleTypeDef *h);

/**
 * @brief  Disable Timer1 counting. Clears T1CON<TMR1ON>.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Stop(void);

/**
 * @brief Atomically read the 16-bit counter (RD16 latches TMR1H on TMR1L read).
 * @return the current 16-bit TMR1 value.
 */
uint16_t EPIC_TIMER1_ReadCounter(void);

/**
 * @brief Atomically write the 16-bit counter (RD16 latches TMR1H on TMR1L write).
 * @param value the 16-bit value to load into the counter.
 */
void EPIC_TIMER1_WriteCounter(uint16_t value);

/**
 * @brief Convert a prescaler enum to its integer ratio (1, 2, 4, 8).
 * @param p the prescaler enum value.
 * @return the integer division ratio (1, 2, 4 or 8).
 */
uint16_t EPIC_TIMER1_PrescalerToRatio(TIMER1_PrescalerTypeDef p);

/**
 * @brief Weak Timer1 ISR, override in user code to add application logic.
 */
void TIMER1_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_TIMER1_H */
