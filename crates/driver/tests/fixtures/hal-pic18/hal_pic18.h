/* Vendored slice of the real epic-hal pic18fxx5x-hal for the PIC18F4550,
 * adapted to compile and run under epic-cc. This is the epic-cc variant of
 * the HAL's SFR/GPIO/Timer0/Timer2/IRQ/WDT layer (the same sources that
 * live in epic-hal's include/epiccc and src/epiccc), reduced to the slice
 * the PIC18 smoke needs: GPIO on RB0, Timer0, Timer2, the IRQ controller,
 * the watchdog refresh, and the 1 ms epic-tick timebase.
 *
 * Adapted around three open backend gaps, exactly as the 887 HAL's
 * epic-cc variants are:
 *   - no i64 struct copies (epic-cc irparse panics on `g_t2_storage = *h`),
 *     so handles are copied field by field;
 *   - no calls through a function pointer (epic-cc#73), so the tick ISR
 *     advances the counter directly instead of through an OverflowCallback;
 *   - no `const` global initializers (`@__const.*`), so handles are built
 *     by field assignment, not `TIMERx_HANDLE_DEFAULT`.
 *
 * The e2e test drives simulated time by asserting the timer flag registers
 * exactly as the 877A blink does: the sim has no timer hardware, so the
 * test sets PIR1/TMR2IF (or INTCON/TMR0IF) and the fixture's main loop
 * clears the flag and advances the counter.
 *
 * SFR addresses and bit masks are from the PIC18F4550 data sheet
 * (DS39632E) via epic-hal's pic18fxx5x_sfr.h.
 */

#ifndef HAL_PIC18_H
#define HAL_PIC18_H

#include <stdint.h>
#include <stdbool.h>

#define EPIC_BIT(n) (1U << (n))
#define EPIC_BIT_SET(reg, mask) ((reg) |= (uint8_t)(mask))
#define EPIC_BIT_CLR(reg, mask) ((reg) &= (uint8_t)~(mask))

#define EPIC_REG8(addr) (*(volatile uint8_t *)(uintptr_t)(addr))
#define EPIC_SFR(addr)  (*(volatile uint8_t *)(uintptr_t)(addr))

/* PIC18F4550 SFR addresses (DS39632E). */
#define PIC_REG_PORTB  0xF81U
#define PIC_REG_LATB   0xF8AU
#define PIC_REG_TRISB  0xF93U
#define PIC_REG_PIE1   0xF9DU
#define PIC_REG_PIR1   0xF9EU
#define PIC_REG_PIE2   0xFA0U
#define PIC_REG_PIR2   0xFA1U
#define PIC_REG_T2CON  0xFCAU
#define PIC_REG_PR2    0xFCBU
#define PIC_REG_TMR2   0xFCCU
#define PIC_REG_INTCON 0xFF2U

/* INTCON bits (DS39632E Register 9-1). */
#define PIC_INTCON_GIE     EPIC_BIT(7)
#define PIC_INTCON_TMR0IE  EPIC_BIT(5)
#define PIC_INTCON_TMR0IF  EPIC_BIT(2)

/* PIE1 bits (DS39632E Register 9-8). */
#define PIC_PIE1_TMR2IE EPIC_BIT(1)

/* PIR1 bits (DS39632E Register 9-10). */
#define PIC_PIR1_TMR2IF EPIC_BIT(1)

/* T2CON bits (DS39632E Register 12-2). */
#define PIC_T2CON_TOUTPS_MASK 0x78U
#define PIC_T2CON_TMR2ON      EPIC_BIT(2)
#define PIC_T2CON_T2CKPS_MASK 0x03U

/* Status codes (epic-common core/hal_status.h). */
typedef enum {
    EPIC_OK      = 0x00U,
    EPIC_ERROR   = 0x01U,
    EPIC_BUSY    = 0x02U,
    EPIC_TIMEOUT = 0x03U,
    EPIC_INVALID = 0x04U
} EPIC_StatusTypeDef;

#endif /* PIC_HAL_PIC18_H */
