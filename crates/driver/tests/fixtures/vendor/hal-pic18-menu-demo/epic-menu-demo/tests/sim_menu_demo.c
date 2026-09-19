/**
 * @brief   Bounded, self-reporting mdb sim gate for epic-menu-demo.
 *
 * Runs the exact same menu_demo_core.c logic the real target uses, but
 * instead of real button hardware (which mdb SIM cannot toggle without
 * external register-poke scripting), a scheduled taskmgr task injects a
 * fixed, deterministic sequence of button events straight into
 * menu_demo_push_event at known tick counts. Tick-driven scheduling
 * (not instruction-count-driven busy waits) makes the resulting event
 * timeline, screen transitions, and EEPROM write count reproducible
 * regardless of which toolchain compiled the binary: task periods are
 * counted in scheduler ticks, and the EEPROM's self-timed write
 * duration is a fixed real-time constant, so both land on the same
 * tick number in an XC8 build and an epic-cc build alike. That is the
 * property scripts/compare-toolchains.sh's UART trace diff relies on.
 *
 * Timer0 tick config mirrors epic-taskmgr's own proven sim gate
 * (tests/sim_taskmgr.c): 1:16 prescaler, reload 0, ~341 us/tick at
 * 48 MHz, well past the PIC18-vs-PIC16 SIM caveats documented there.
 */

#include "menu_demo_core.h"

#include "core/epic_harness.h"
#include "epic_hal.h"
#include "epic_taskmgr.h"

#include <stdint.h>

/** Loop-iteration bound for epic_taskmgr_run()'s harness-driven loop
 * (core/epic_harness.h: not a real-time unit), empirically calibrated:
 * this firmware's per-round cost is far heavier than epic-taskmgr's
 * own sim gate's 1500. 200 is the smallest value observed to reliably
 * let the scripted sequence, the EEPROM write pair (via the
 * eeprom_writes mdb poke cycles, see scripts/sim-mdb-run.sh), and the
 * final ticks>=100 check all complete -- see the ADC comment below for
 * why this gate needs no more than a 60s wait_ms budget. */
#define SIM_ITERATIONS 200UL

#define TICK_RELOAD    0U
#define TICK_PRESCALER TIMER0_PRESCALER_1_16

#define TASK_PERIOD_UI        1U
#define TASK_PERIOD_EEPROM    5U
#define TASK_PERIOD_HEARTBEAT 50U
#define TASK_PERIOD_STIMULUS  1U

/** Scripted stimulus: (tick threshold, event). Cycles STATUS ->
 *  BRIGHTNESS -> ABOUT -> STATUS (3 SELECTs) with two brightness
 *  increments in between, spaced widely enough that each UI task round
 *  (period 1) drains one event before the next arrives. */
typedef struct {
    uint16_t     tick;
    menu_event_t event;
} stimulus_t;

static const stimulus_t SCRIPT[] = {
    { 5U,  MENU_EVENT_SELECT }, /* STATUS -> BRIGHTNESS */
    { 10U, MENU_EVENT_UP },     /* brightness 5 -> 6 */
    { 15U, MENU_EVENT_UP },     /* brightness 6 -> 7 */
    { 20U, MENU_EVENT_SELECT }, /* BRIGHTNESS -> ABOUT */
    { 25U, MENU_EVENT_SELECT }, /* ABOUT -> STATUS */
};
#define SCRIPT_LEN (sizeof(SCRIPT) / sizeof(SCRIPT[0]))

/** Round-actual tick each scripted event was pushed at. Threshold ticks
 *  are only 5 apart, but a round that does real LCD/ADC/EEPROM work
 *  costs enough simulated time that several thresholds can fall due in
 *  a single scheduler round (verified: rounds sometimes lag Timer0 by
 *  40+ ticks under mdb SIM), so this can legitimately differ from
 *  SCRIPT[i].tick -- logged for visibility, not asserted against. */
static uint8_t g_script_idx;
static uint16_t g_fire_tick[SCRIPT_LEN];

/**
 * @brief taskmgr task: push any scripted events now due, in tick order.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
static void task_stimulus(void *arg)
{
    (void)arg;
    uint16_t ticks = epic_taskmgr_ticks();
    while (g_script_idx < SCRIPT_LEN && ticks >= SCRIPT[g_script_idx].tick)
    {
        menu_demo_push_event(SCRIPT[g_script_idx].event);
        g_fire_tick[g_script_idx] = ticks;
        g_script_idx++;
    }
}

/** @brief Log each fired event's actual tick as 4 hex digits, space-separated. */
static void log_fire_ticks(void)
{
    static const char hx[] = "0123456789ABCDEF";
    static char buf[6];
    uint8_t i;
    for (i = 0; i < g_script_idx; i++)
    {
        uint16_t v = g_fire_tick[i];
        buf[0] = hx[(v >> 12) & 0xFU];
        buf[1] = hx[(v >> 8) & 0xFU];
        buf[2] = hx[(v >> 4) & 0xFU];
        buf[3] = hx[v & 0xFU];
        buf[4] = ' ';
        buf[5] = '\0';
        epic_harness_log(buf);
    }
    epic_harness_log("\n");
}

static uint16_t g_fail;

/**
 * @brief Record a check failure and log its index as two hex digits.
 * @param idx check index (see the CHECK call sites in main)
 */
static void fail(uint8_t idx)
{
    /* Static RAM buffer, not stack locals or const pointers: the
     * epic-cc build has no const-address form. */
    static const char hx[] = "0123456789ABCDEF";
    static char c[5];
    g_fail++;
    c[0] = 'F';
    c[1] = hx[(idx >> 4) & 0xFU];
    c[2] = hx[idx & 0xFU];
    c[3] = '.';
    c[4] = ' ';
    epic_harness_log(c);
}

#define CHECK(cond, idx) do { if (!(cond)) fail(idx); } while (0)

/**
 * @brief Run the scripted button sequence and report PASS/FAIL.
 * @return 0 on pass, 1 on fail (see core/epic_harness.h)
 */
int main(void)
{
    epic_harness_init(SIM_ITERATIONS);
    menu_demo_init();

    epic_taskmgr_init();
    epic_taskmgr_spawn(task_stimulus,            NULL, TASK_PERIOD_STIMULUS,  0U);
    /* menu_demo_task_adc deliberately not spawned: MPLAB SIM logs a
     * console warning on every conversion (no real analog stimulus on
     * AN0), and that I/O overhead alone starved this gate for 10+
     * minutes despite correct, complete application state; removing it
     * reaches PASS in under a minute. The ADC is still exercised at
     * init and polled on real hardware (example_menu_demo.c). */
    epic_taskmgr_spawn(menu_demo_task_ui,        NULL, TASK_PERIOD_UI,        2U);
    epic_taskmgr_spawn(menu_demo_task_eeprom,    NULL, TASK_PERIOD_EEPROM,    3U);
    epic_taskmgr_spawn(menu_demo_task_heartbeat, NULL, TASK_PERIOD_HEARTBEAT, 4U);

    epic_taskmgr_attach_timer0(TICK_RELOAD, TICK_PRESCALER);
    EPIC_IRQ_Restore(1);

    epic_taskmgr_run(); /* harness-bounded on the sim target */

    log_fire_ticks();

    CHECK(g_script_idx == SCRIPT_LEN, 0x00);              /* full script ran */
    CHECK(menu_demo_screen() == MENU_SCREEN_STATUS, 0x01); /* 3 SELECTs cycled back */
    CHECK(menu_demo_brightness() == 7U, 0x02);             /* 5 default + 2 UP */
    CHECK(menu_demo_eeprom_writes() >= 2U, 0x03);          /* at least one write pair */
    CHECK((menu_demo_eeprom_writes() % 2U) == 0U, 0x04);   /* always magic+brightness pairs */
    CHECK(epic_taskmgr_ticks() >= 100U, 0x05);              /* tick source ran the whole budget */

    return epic_harness_report(g_fail == 0U);
}
