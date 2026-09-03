/**
 * 1 ms timebase on the HAL's Timer2, family-agnostic. The tick ISR is
 * the handle's `OverflowCallback`: the HAL's own `TIMER2_IRQHandler`
 * clears TMR2IF and calls it, so this module never redefines the
 * handler.
 */

#include "epic_tick.h"
#include "peripherals/hal_timer2.h"   /* family-neutral shim -> pic*_timer2.h */
#include "core/hal_irq.h"             /* EPIC_IRQ_Disable / Restore            */
#include "core/epic_harness.h"        /* epic_harness_tick (host sim pump)    */
static volatile uint32_t g_tick_ms = 0u;

/**
 * @brief  Timer2 overflow callback: advance the 1 ms tick counter.
 */
static void epic_tick_on_overflow(void)
{
    g_tick_ms++;
}

/**
 * @brief  Pick the Timer2 configuration whose period is closest to 1 ms.
 * @param fosc_hz the system oscillator frequency in Hz.
 * @param pr2     where the best PR2 reload value is written.
 * @param pre     where the best prescaler setting is written.
 * @param post    where the best postscaler setting is written.
 */
static void compute_period(uint32_t fosc_hz, uint8_t *pr2,
                           TIMER2_PrescalerTypeDef *pre,
                           TIMER2_PostscalerTypeDef *post)
{
    uint32_t target = fosc_hz / 4000u;       /* instruction cycles per 1 ms */
    if (target == 0u) { target = 1u; }

    static const TIMER2_PrescalerTypeDef pre_enum[3] = {
        TIMER2_PRESCALER_1_16, TIMER2_PRESCALER_1_4, TIMER2_PRESCALER_1_1 };
    static const uint16_t pre_ratio[3] = { 16u, 4u, 1u };

    uint32_t best_err = 0xFFFFFFFFu;
    uint8_t best_pr2 = 0xFFu;
    TIMER2_PrescalerTypeDef best_pre = TIMER2_PRESCALER_1_1;
    TIMER2_PostscalerTypeDef best_post = TIMER2_POSTSCALER_1_1;

    for (int i = 0; i < 3; i++) {
        uint32_t p = pre_ratio[i];
        for (uint32_t q = 1u; q <= 16u; q++) {
            uint32_t pq = p * q;
            uint32_t n = target / pq;            /* n = PR2+1 */
            /* try n and n+1 (the floor and ceil of target/pq), clamped 1..256 */
            for (uint32_t k = 0u; k < 2u; k++) {
                uint32_t nn = n + k;
                if (nn < 1u || nn > 256u) { continue; }
                uint32_t cand = pq * nn;
                uint32_t err = (cand >= target) ? (cand - target) : (target - cand);
                if (err < best_err) {
                    best_err  = err;
                    best_pr2  = (uint8_t)(nn - 1u);
                    best_pre  = pre_enum[i];
                    best_post = (TIMER2_PostscalerTypeDef)(q - 1u);
                }
            }
        }
    }
    *pr2  = best_pr2;
    *pre  = best_pre;
    *post = best_post;
}

/**
 * @brief  Start the 1 ms timebase on Timer2 (target implementation).
 * @param fosc_hz the system oscillator frequency in Hz.
 */
void epic_tick_init(uint32_t fosc_hz)
{
    uint8_t pr2;
    TIMER2_PrescalerTypeDef pre;
    TIMER2_PostscalerTypeDef post;
    compute_period(fosc_hz, &pr2, &pre, &post);

    g_tick_ms = 0u;
    /* The handle is a local, not a static: with the Init inlined, clang
     * folds every field load and the callback store lands as a named
     * literal, which is how the epic-cc cross-context analysis resolves
     * the ISR's dispatch candidates (the same shape as the timer0
     * driver's inlined Init, epic-hal#105). A static handle would leave
     * the store a runtime load-copy and the tick's Timer2 callback would
     * trap on the epic-cc path (epic-hal#86). */
    TIMER2_HandleTypeDef h = TIMER2_HANDLE_DEFAULT;
    h.Prescaler        = pre;
    h.Postscaler       = post;
    h.Period           = pr2;
    h.OverflowCallback = epic_tick_on_overflow;
    EPIC_TIMER2_Init(&h);
    EPIC_TIMER2_Start(&h);
    /* EPIC_TIMER2_Init only arms Timer2's own source enable; the global
     * interrupt enable is separate, so without this the ISR never fires
     * and epic_tick_delay_ms spins forever (epic-common/MANUAL.md §6-7). */
    EPIC_IRQ_Restore(1);
}

/**
 * @brief  Read the 1 ms tick counter (target implementation).
 * @return the millisecond tick count.
 */
uint32_t epic_tick_get(void)
{
    /* Double-read retry: the ISR can update the 32-bit counter between
     * the 8-bit byte reads, and disabling GIE is not reliable under
     * MPLAB SIM (a request latched while GIE was set can still vector,
     * and can leave GIE cleared, killing the tick). The retry is
     * race-free on real silicon too, at the cost of an occasional
     * re-read. */
    uint32_t a, b;
    do {
        a = g_tick_ms;
        b = g_tick_ms;
    } while (a != b);
    return a;
}

/**
 * @brief  Milliseconds elapsed since a tick timestamp (target impl).
 * @param t0 an earlier `epic_tick_get()` timestamp.
 * @return `epic_tick_get() - t0`, wraparound-safe.
 */
uint32_t epic_tick_elapsed_since(uint32_t t0)
{
    return epic_tick_get() - t0;               /* unsigned: wraparound-safe   */
}

/**
 * @brief  Block for `ms` milliseconds (target implementation).
 * @param ms how long to block, in milliseconds.
 */
void epic_tick_delay_ms(uint32_t ms)
{
    uint32_t t0 = epic_tick_get();
    while (epic_tick_elapsed_since(t0) < ms) {
        epic_harness_tick();                   /* host: pump sim; target: no-op */
    }
}
