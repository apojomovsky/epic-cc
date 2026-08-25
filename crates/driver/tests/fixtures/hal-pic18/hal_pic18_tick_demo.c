/* epic-tick on the 4550: the 1 ms Timer2 timebase. This is the epic-cc
 * build of the HAL's epic-tick module + its example program, adapted to
 * the slice. `main` starts the tick, delays 10 ms then 5 ms, and checks
 * the elapsed counts land within one tick of the requested value (the
 * same checks as epic-tick/tests/sim_tick.c). The sim drives the tick:
 * the e2e asserts PIR1<TMR2IF> each time it wants a millisecond to pass,
 * and `epic_tick_delay_ms` advances `g_tick_ms` when it sees the flag.
 * `g_tick_ms` is a volatile global the e2e reads from the address map.
 */

#include "hal_pic18.h"

#define FOSC_HZ 48000000UL

extern void epic_tick_init(uint32_t fosc_hz);
extern uint32_t epic_tick_get(void);
extern uint32_t epic_tick_elapsed_since(uint32_t t0);
extern void epic_tick_delay_ms(uint32_t ms);

/* Observed results, non-static so the e2e test can read them from the
 * address map. */
volatile uint32_t g_tick_e10 = 0u;
volatile uint32_t g_tick_e5 = 0u;
volatile uint32_t g_tick_result = 0u;

int main(void)
{
    epic_tick_init(FOSC_HZ);

    uint32_t t0 = epic_tick_get();
    epic_tick_delay_ms(10u);
    uint32_t e10 = epic_tick_get() - t0;
    g_tick_e10 = e10;

    uint32_t s = epic_tick_get();
    epic_tick_delay_ms(5u);
    uint32_t e5 = epic_tick_elapsed_since(s);
    g_tick_e5 = e5;

    /* Report: 0 (pass) if both delays landed within one tick. */
    int ok = (e10 >= 10u) && (e10 <= 12u) && (e5 >= 5u) && (e5 <= 7u);
    g_tick_result = (uint32_t)ok;
    return ok ? 0 : 1;
}
