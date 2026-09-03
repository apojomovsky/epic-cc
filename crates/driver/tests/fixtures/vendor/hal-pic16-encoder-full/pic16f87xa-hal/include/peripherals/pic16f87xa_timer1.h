/* Timer1 driver, 16-bit timer/counter. Source: DS39582B §6.0; full
 * reference: MANUAL.md §11. T1OSCEN enables a 32.768 kHz crystal on
 * RC0/RC1 for RTC use; the CCP special-event trigger (§6.6) can reset
 * TMR1H:L, configured by the CCP driver, not here. */

#ifndef PIC16F87XA_TIMER1_H
#define PIC16F87XA_TIMER1_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief Timer1 clock source (T1CON<TMR1CS>, DS39582B Register 6-1).
 */
typedef enum {
    TIMER1_CLOCK_INTERNAL  = 0x0U,   /**< Fosc/4 (timer mode). */
    TIMER1_CLOCK_EXTERNAL  = 0x1U,   /**< External pin or T1OSC. */
} TIMER1_ClockSourceTypeDef;

/**
 * @brief External-clock synchronisation (T1CON<T1SYNC>, DS39582B §6.3/§6.4).
 *        In timer mode (internal clock) this bit is ignored.
 *
 *        Asynchronous mode is required for Sleep-time counting, and is
 *        the default when the T1OSC crystal oscillator is enabled.
 */
typedef enum {
    TIMER1_SYNC_EXTERNAL   = 0x0U,   /**< T1SYNC = 0, synchronise to Fosc. */
    TIMER1_ASYNC_EXTERNAL  = 0x1U,   /**< T1SYNC = 1, free-running. */
} TIMER1_ClockSyncTypeDef;

/**
 * @brief T1OSC crystal-oscillator enable (T1CON<T1OSCEN>, DS39582B §6.5).
 *        Requires a 32.768 kHz crystal on T1OSI/T1OSO; the resulting
 *        1 Hz tick rate is the standard RTC time base.
 */
typedef enum {
    TIMER1_OSCILLATOR_OFF  = 0x0U,
    TIMER1_OSCILLATOR_ON   = 0x1U,
} TIMER1_OscillatorTypeDef;

/**
 * @brief Prescaler ratio (T1CON<T1CKPS1:T1CKPS0>, DS39582B Register 6-1).
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
    .Prescaler        = TIMER1_PRESCALER_1_1,                           \
    .ReloadValue      = 0x0000U,                                        \
    .OverflowCallback = NULL,                                           \
}

/**
 * @brief  Initialize Timer1 from the handle. Programs T1CON (clock
 *         source, sync, oscillator, prescaler) and INTCON<PIE1/TMR1IE>,
 *         and loads ReloadValue into TMR1H:L.
 * @param h handle with ClockSource, ClockSync, Oscillator, Prescaler,
 *        ReloadValue, OverflowCallback.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Init(const TIMER1_HandleTypeDef *h);

/**
 * @brief  De-initialize Timer1. Disables the overflow interrupt and
 *         returns T1CON to reset.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER1_DeInit(void);

/**
 * @brief  Start Timer1 counting. Writes `h->ReloadValue` into
 *         TMR1H:L and sets TMR1ON.
 * @param h handle whose ReloadValue is loaded into TMR1H:L.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Start(const TIMER1_HandleTypeDef *h);

/**
 * @brief  Stop Timer1 counting. Clears TMR1ON.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER1_Stop(void);

/**
 * @brief Atomically read the 16-bit counter value.
 * @return the current 16-bit TMR1H:L value.
 */
uint16_t EPIC_TIMER1_ReadCounter(void);

/**
 * @brief Atomically write the 16-bit counter value.
 * @param value the 16-bit value to load into TMR1H:L.
 */
void EPIC_TIMER1_WriteCounter(uint16_t value);

/**
 * @brief Convert a prescaler enum to its integer ratio (1, 2, 4, 8).
 * @param p the prescaler enum value.
 * @return the integer prescaler ratio (1, 2, 4 or 8).
 */
uint16_t EPIC_TIMER1_PrescalerToRatio(TIMER1_PrescalerTypeDef p);

/**
 * @brief Weak Timer1 ISR, override in user code to add application logic.
 */
void TIMER1_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_TIMER1_H */
