/* Vendored slice of the real epic-hal pic16f87xa-hal Timer0 driver
 * (DS39582B §5.0), adapted to compile under epic-cc. Reduced to the
 * callback surface: internal Fosc/4, 1:256 prescaler, reload 0. The
 * callback slot is a 1-byte global the ISR reads and invokes; Init and
 * Start are header static inlines (as in the guard-free HAL,
 * epic-hal#105) so the caller's TU folds the handle-field load to a
 * named literal and epic-cc resolves the cross-context store (ADR-024).
 */

#include "hal_pic16_timer0.h"

/* The ISR's owned callback slot. Non-static so the header inline can
 * store to it from any TU (the HAL's `extern` declaration). */
void (*g_t0_overflow_cb)(void) = NULL;

/**
 * @brief Weak Timer0 ISR: clears TMR0IF and fires the overflow
 *        callback. The e2e fires the interrupt and asserts the
 *        callback ran (the headline HAL API, epic-hal#105).
 */
void TIMER0_IRQHandler(void)
{
    /* TMR0IF is INTCON bit 2 (DS39582B §14.11). */
    if (!(EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
    if (g_t0_overflow_cb) g_t0_overflow_cb();
}
