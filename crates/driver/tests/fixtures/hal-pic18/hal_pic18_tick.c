/* Vendored slice of the real epic-hal epic-tick module, the 1 ms timebase
 * on Timer2, adapted to compile under epic-cc. The real target driver
 * advances `g_tick_ms` from a Timer2 overflow callback invoked through a
 * function pointer (open epic-cc#73); under epic-cc the tick advances
 * directly in `epic_tick_delay_ms`, which polls TMR2IF, the same idiom the
 * 887 HAL's epic-cc example uses for the blink.
 *
 * `compute_period` in the real module searches every (prescaler,
 * postscaler, PR2) triple for the closest 1 ms period; that triple-nested
 * loop lowers to a body whose conditional branch exceeds the PIC18
 * 128-word near-branch limit, so the epic-cc build hardcodes the exact
 * result for the fixed 48 MHz clock. 48 MHz / 4 = 12 MHz instruction
 * clock, so 1 ms is 12000 instruction cycles. A 1:16 prescaler times a
 * 1:3 postscaler divides by 48, giving PR2 + 1 = 12000 / 48 = 250, i.e.
 * PR2 = 249 with an exact 1 ms period (PR2 is 8-bit, so the divisor must
 * be a factor of 12000; 48 is one such factor).
 */


#include "hal_pic18.h"

static volatile uint32_t g_tick_ms = 0u;

void epic_tick_init(uint32_t fosc_hz);

uint32_t epic_tick_get(void)
{
    /* Single read; the sim drives the tick between steps, so there is no
     * ISR racing a 32-bit read here. */
    return g_tick_ms;
}

uint32_t epic_tick_elapsed_since(uint32_t t0)
{
    return epic_tick_get() - t0;
}

void epic_tick_init(uint32_t fosc_hz)
{
    (void)fosc_hz;

    g_tick_ms = 0u;
    /* Program Timer2 directly (no handle struct copy, avoiding the i64
     * struct-copy backend gap): 1:16 prescaler, 1:3 postscaler, PR2 such
     * that the period is exactly 1 ms at 48 MHz (see the file header). */
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_T2CON), PIC_T2CON_TMR2ON);
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_PIE1), PIC_PIE1_TMR2IE);
    EPIC_REG8(PIC_REG_PR2) = 249u; /* PR2 = 249, TMR2IF every 1 ms */
    uint8_t v = (uint8_t)((2u & 0xFu) << 3); /* postscaler 1:3 -> N=2 at bits 6:3 */
    v |= PIC_T2CON_TMR2ON;
    v |= 2u; /* prescaler 1:16 (T2CKPS = 2) */
    EPIC_REG8(PIC_REG_T2CON) = v;
    EPIC_BIT_SET(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_GIE);
}

void epic_tick_delay_ms(uint32_t ms)
{
    uint32_t target = g_tick_ms + ms;
    while (g_tick_ms < target) {
        /* Pump simulated time: on the sim the e2e drives TMR2IF. */
        if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR2IF) {
            EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_TMR2IF);
            g_tick_ms++;
        }
    }
}
