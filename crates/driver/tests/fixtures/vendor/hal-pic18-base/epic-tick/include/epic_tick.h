/**
 * Family-agnostic 1 ms timebase (`epic_tick_get`/`epic_tick_delay_ms`,
 * the STM32Cube `HAL_GetTick`/`HAL_Delay` equivalent) on the HAL's
 * auto-reload Timer2.
 */

#ifndef EPIC_TICK_H
#define EPIC_TICK_H

#include <stdint.h>

/**
 * @brief  Start the 1 ms timebase: configures Timer2 for the closest
 *         achievable 1 ms period from @p fosc_hz and enables its ISR.
 *         Call once at startup.
 * @param  fosc_hz  the system oscillator frequency in Hz (e.g. 20000000UL).
 */
void epic_tick_init(uint32_t fosc_hz);

/**
 * @brief  Read the elapsed milliseconds since `epic_tick_init`.
 *         Monotonic; wraps every ~49.7 days (2^32 ms). The 32-bit read
 *         is race-free against a mid-read ISR update (double-read
 *         retry).
 * @return the millisecond tick count.
 */
uint32_t epic_tick_get(void);

/**
 * @brief  Block for @p ms milliseconds. On the host sim this pumps
 *         `epic_harness_tick()` so simulated time advances; on a real target
 *         it spins while the Timer2 ISR advances the counter. Guarantees at
 *         least @p ms (may overshoot by up to ~1 tick).
 * @param ms how long to block, in milliseconds.
 */
void epic_tick_delay_ms(uint32_t ms);

/**
 * @brief  Non-blocking elapsed-time helper: `epic_tick_get() - t0`, the
 *         idiom for `if (epic_tick_elapsed_since(t0) >= timeout)` without
 *         blocking. Wraparound-safe (unsigned subtraction).
 * @param t0 a timestamp captured earlier with `epic_tick_get()`.
 * @return milliseconds elapsed since `t0`.
 */
uint32_t epic_tick_elapsed_since(uint32_t t0);

#endif /* EPIC_TICK_H */
