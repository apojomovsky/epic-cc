/*
 * Timer0 driver, 8-bit or 16-bit timer/counter with a dedicated
 * prescaler (DS39632E §11.0). PIC18's Timer0 is controlled by the
 * dedicated T0CON register (not PIC16's OPTION_REG) and has its own
 * prescaler, not shared with the WDT. Default is 8-bit (T08BIT=1),
 * matching PIC16's model so existing caller code is a drop-in.
 */

#ifndef PIC18FXX5X_TIMER0_H
#define PIC18FXX5X_TIMER0_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief Timer0 counter width (T0CON<T08BIT>, DS39632E Register 11-1).
 *        T08BIT = 1 is 8-bit (the PIC16-compatible mode, the default);
 *        T08BIT = 0 is 16-bit (TMR0H:TMR0L).
 */
typedef enum {
    TIMER0_BITMODE_16BIT = 0x0U,  /**< T08BIT = 0, 16-bit counter. */
    TIMER0_BITMODE_8BIT  = 0x1U,  /**< T08BIT = 1, 8-bit counter.   */
} TIMER0_BitModeTypeDef;

/**
 * @brief Timer0 clock source (T0CON<T0CS>, DS39632E §11.0, Reg 11-1).
 */
typedef enum {
    TIMER0_CLOCK_INTERNAL = 0x0U,   /**< Fosc/4, T0CS = 0. */
    TIMER0_CLOCK_EXTERNAL = 0x1U,   /**< T0CKI pin, T0CS = 1. */
} TIMER0_ClockSourceTypeDef;

/**
 * @brief Timer0 external-clock edge (T0CON<T0SE>, DS39632E §11.0).
 *        Ignored in internal-clock mode.
 */
typedef enum {
    TIMER0_EDGE_RISING  = 0x0U,     /**< Increment on T0CKI rising edge.  */
    TIMER0_EDGE_FALLING = 0x1U,     /**< Increment on T0CKI falling edge. */
} TIMER0_ClockEdgeTypeDef;

/**
 * @brief Timer0 prescaler ratio. Encoded as the value to load into
 *        T0CON<T0PS2:T0PS0> (DS39632E Table 11-1).
 *
 *        PS2:PS0=000 is 1:2 for Timer0, NOT 1:1; the 1:1 option is "no
 *        prescaler", selected by NOT assigning the prescaler to Timer0
 *        (PSA = 1, TMR0 gets the raw clock).
 */
typedef enum {
    TIMER0_PRESCALER_1_2    = 0x0U,  /**< 1:2, T0PS = 000. */
    TIMER0_PRESCALER_1_4    = 0x1U,  /**< 1:4, T0PS = 001. */
    TIMER0_PRESCALER_1_8    = 0x2U,  /**< 1:8, T0PS = 010. */
    TIMER0_PRESCALER_1_16   = 0x3U,  /**< 1:16, T0PS = 011. */
    TIMER0_PRESCALER_1_32   = 0x4U,  /**< 1:32, T0PS = 100. */
    TIMER0_PRESCALER_1_64   = 0x5U,  /**< 1:64, T0PS = 101. */
    TIMER0_PRESCALER_1_128  = 0x6U,  /**< 1:128, T0PS = 110. */
    TIMER0_PRESCALER_1_256  = 0x7U,  /**< 1:256, T0PS = 111. */
} TIMER0_PrescalerTypeDef;

/**
 * @brief  Driver handle (Cube-style). Same fields as the PIC16 handle plus
 *         @ref Mode (the PIC18-only 8/16-bit select, defaulting to 8-bit).
 */
typedef struct {
    TIMER0_BitModeTypeDef      Mode;            /**< 8-bit (default) or 16-bit. */
    TIMER0_ClockSourceTypeDef  ClockSource;     /**< Internal or T0CKI. */
    TIMER0_ClockEdgeTypeDef    ClockEdge;       /**< T0CKI edge (or rising). */
    TIMER0_PrescalerTypeDef    Prescaler;       /**< 1:2..1:256. */
    bool                       PrescalerAssigned; /**< true = prescaler -> TMR0. */
    uint8_t                    ReloadValue;    /**< Low-byte start (8-bit), or TMR0L. */
    /** @brief  Optional overflow callback. Called from interrupt context
     *         on every overflow. */
    void (*OverflowCallback)(void);
} TIMER0_HandleTypeDef;

/**
 * @brief  Default initialiser: 8-bit mode, internal Fosc/4, prescaler 1:256,
 *         no callback. The 8-bit default keeps it a drop-in for PIC16
 *         caller code (the task manager).
 */
#define TIMER0_HANDLE_DEFAULT {                                         \
    .Mode               = TIMER0_BITMODE_8BIT,                          \
    .ClockSource        = TIMER0_CLOCK_INTERNAL,                        \
    .ClockEdge          = TIMER0_EDGE_RISING,                           \
    .Prescaler          = TIMER0_PRESCALER_1_256,                       \
    .PrescalerAssigned  = true,                                         \
    .ReloadValue        = 0x00U,                                        \
    .OverflowCallback   = NULL,                                         \
}

/**
 * @brief  Configure Timer0 from the handle. Programs T0CON and
 *         INTCON<TMR0IE>. Does not start the timer, call @ref
 *         EPIC_TIMER0_Start afterwards.
 *
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 * @param h the Timer0 handle describing the desired configuration.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Init(const TIMER0_HandleTypeDef *h);

/**
 * @brief  Disable Timer0 counting and clear TMR0IF.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER0_DeInit(void);

/**
 * @brief  Timer0 weak ISR. Forward-declared so user code can override it
 *         (Cube-style). When the user provides an `OverflowCallback`
 *         through `EPIC_TIMER0_Init`, the default implementation invokes
 *         it; otherwise it just clears TMR0IF and returns.
 */
void TIMER0_IRQHandler(void) EPIC_WEAK;

/**
 * @brief  Enable Timer0 counting. Sets T0CON<T0CS> / T0SE / T08BIT / PSA /
 *         T0PS, writes `h->ReloadValue` into TMR0L (and TMR0H in 16-bit
 *         then sets TMR0ON.
 * @param h the handle used to configure the timer.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Start(const TIMER0_HandleTypeDef *h);

/**
 * @brief Disable Timer0 counting. Clears T0CON<TMR0ON> -> Timer0 halted.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_TIMER0_Stop(void);

/**
 * @brief Read the current counter low byte (TMR0L). In 16-bit mode this
 *        latches TMR0H on the hardware; only the low byte is returned
 *        (the API is 8-bit to match the PIC16 contract). Use the SFR
 *        directly for the full 16-bit value if needed.
 * @return the current TMR0L value.
 */
uint8_t EPIC_TIMER0_ReadCounter(void);

/**
 * @brief Write `value` to TMR0L (the low byte / 8-bit counter).
 * @param value the value to load into the counter.
 */
void EPIC_TIMER0_WriteCounter(uint8_t value);

/**
 * @brief  Convert a prescaler enum to its integer ratio (1, 2, 4, ..., 256).
 * @param p the prescaler enum value.
 * @return the integer division ratio (2..256; 1:1 if unassigned).
 */
uint16_t EPIC_TIMER0_PrescalerToRatio(TIMER0_PrescalerTypeDef p);

#endif /* PIC18FXX5X_TIMER0_H */
