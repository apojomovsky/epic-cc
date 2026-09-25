/**
 * @brief Bounded, self-reporting mdb sim gate for epic-control-demo.
 *
 * Runs the exact same control_demo_core.c logic the real target uses,
 * but MPLAB SIM can neither inject UART RX bytes nor drive AN0, so a
 * scheduled taskmgr task feeds both deterministically: console command
 * bytes go straight into control_demo_inject_console_byte and plant
 * readings into control_demo_inject_adc at known tick counts. The
 * console task still drains the (empty, in sim) UART RX ring every
 * tick, so the real-HW path is compiled in and exercised for its
 * empty-ring contract. Tick-driven scheduling keeps the command
 * timeline and EEPROM write count reproducible across toolchains.
 *
 * Timer0 tick config mirrors epic-taskmgr's own proven sim gate
 * (tests/sim_taskmgr.c): 1:16 prescaler, reload 0, ~341 us/tick at
 * 48 MHz.
 */

#include "control_demo_core.h"

#include "core/epic_harness.h"
#include "epic_hal.h"
#include "epic_taskmgr.h"

#include <stdint.h>

/** Loop-iteration bound for epic_taskmgr_run()'s harness-driven loop
 * (core/epic_harness.h: not a real-time unit). Ticks race far ahead of
 * rounds under SIM (tens of thousands per gate), so the script, the
 * filter convergence, and the EEPROM issue/complete cycle all finish
 * early; 120 rounds keeps the whole gate, report included, inside a
 * 60 s wait budget where 200 (the menu demo's bound) overruns it. */
#define SIM_ITERATIONS 120UL
#define TASK_PERIOD_STIMULUS  1U
#define TICK_RELOAD    0U
#define TICK_PRESCALER TIMER0_PRESCALER_1_16

#define TASK_PERIOD_CONTROL   5U
#define TASK_PERIOD_CONSOLE   1U
#define TASK_PERIOD_EEPROM    5U
#define TASK_PERIOD_HEARTBEAT 50U

#define STIM_CONSOLE 0U
#define STIM_ADC     1U

/** Scripted stimulus: (tick threshold, kind, value). Console bytes
 * spell full command lines one byte per tick; the ADC entries step the
 * plant 500 -> 600 raw (x4 oversampling: 2000 -> 2400 measured). The
 * out-of-range `set sp 5000` must be rejected, leaving 2800 in force. */
typedef struct {
    uint16_t tick;
    uint8_t  kind;
    uint16_t value;
} stimulus_t;

static const stimulus_t SCRIPT[] = {
    { 2U,  STIM_ADC, 500U },
    { 5U,  STIM_CONSOLE, 'g' },
    { 6U,  STIM_CONSOLE, 'e' },
    { 7U,  STIM_CONSOLE, 't' },
    { 8U,  STIM_CONSOLE, '\n' },
    { 10U, STIM_CONSOLE, 's' },
    { 11U, STIM_CONSOLE, 'e' },
    { 12U, STIM_CONSOLE, 't' },
    { 13U, STIM_CONSOLE, ' ' },
    { 14U, STIM_CONSOLE, 's' },
    { 15U, STIM_CONSOLE, 'p' },
    { 16U, STIM_CONSOLE, ' ' },
    { 17U, STIM_CONSOLE, '2' },
    { 18U, STIM_CONSOLE, '8' },
    { 19U, STIM_CONSOLE, '0' },
    { 20U, STIM_CONSOLE, '0' },
    { 21U, STIM_CONSOLE, '\n' },
    { 24U, STIM_CONSOLE, 's' },
    { 25U, STIM_CONSOLE, 'e' },
    { 26U, STIM_CONSOLE, 't' },
    { 27U, STIM_CONSOLE, ' ' },
    { 28U, STIM_CONSOLE, 'k' },
    { 29U, STIM_CONSOLE, 'p' },
    { 30U, STIM_CONSOLE, ' ' },
    { 31U, STIM_CONSOLE, '6' },
    { 32U, STIM_CONSOLE, '4' },
    { 33U, STIM_CONSOLE, '\n' },
    { 36U, STIM_CONSOLE, 's' },
    { 37U, STIM_CONSOLE, 'e' },
    { 38U, STIM_CONSOLE, 't' },
    { 39U, STIM_CONSOLE, ' ' },
    { 40U, STIM_CONSOLE, 'k' },
    { 41U, STIM_CONSOLE, 'i' },
    { 42U, STIM_CONSOLE, ' ' },
    { 43U, STIM_CONSOLE, '4' },
    { 44U, STIM_CONSOLE, '\n' },
    { 47U, STIM_CONSOLE, 's' },
    { 48U, STIM_CONSOLE, 'e' },
    { 49U, STIM_CONSOLE, 't' },
    { 50U, STIM_CONSOLE, ' ' },
    { 51U, STIM_CONSOLE, 'k' },
    { 52U, STIM_CONSOLE, 'd' },
    { 53U, STIM_CONSOLE, ' ' },
    { 54U, STIM_CONSOLE, '3' },
    { 55U, STIM_CONSOLE, '2' },
    { 56U, STIM_CONSOLE, '\n' },
    { 59U, STIM_CONSOLE, 's' },
    { 60U, STIM_CONSOLE, 'a' },
    { 61U, STIM_CONSOLE, 'v' },
    { 62U, STIM_CONSOLE, 'e' },
    { 63U, STIM_CONSOLE, '\n' },
    { 66U, STIM_CONSOLE, 'b' },
    { 67U, STIM_CONSOLE, 'o' },
    { 68U, STIM_CONSOLE, 'g' },
    { 69U, STIM_CONSOLE, 'u' },
    { 70U, STIM_CONSOLE, 's' },
    { 71U, STIM_CONSOLE, '\n' },
    { 74U, STIM_CONSOLE, 's' },
    { 75U, STIM_CONSOLE, 'e' },
    { 76U, STIM_CONSOLE, 't' },
    { 77U, STIM_CONSOLE, ' ' },
    { 78U, STIM_CONSOLE, 's' },
    { 79U, STIM_CONSOLE, 'p' },
    { 80U, STIM_CONSOLE, ' ' },
    { 81U, STIM_CONSOLE, '5' },
    { 82U, STIM_CONSOLE, '0' },
    { 83U, STIM_CONSOLE, '0' },
    { 84U, STIM_CONSOLE, '0' },
    { 85U, STIM_CONSOLE, '\n' },
    { 88U, STIM_ADC, 600U },
    { 91U, STIM_CONSOLE, 'h' },
    { 92U, STIM_CONSOLE, 'e' },
    { 93U, STIM_CONSOLE, 'l' },
    { 94U, STIM_CONSOLE, 'p' },
    { 95U, STIM_CONSOLE, '\n' },
};
#define SCRIPT_LEN (sizeof(SCRIPT) / sizeof(SCRIPT[0]))

/** Round-actual tick each scripted entry was injected at. Threshold
 * ticks are close together but a round doing PID/EEPROM/UART work
 * costs enough simulated time that several thresholds can fall due in
 * one scheduler round, so these can differ from SCRIPT[i].tick. */
static uint8_t g_script_idx;
static uint16_t g_fire_tick[SCRIPT_LEN];

/**
 * @brief taskmgr task: inject any scripted bytes/readings now due.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
static void task_stimulus(void *arg)
{
    (void)arg;
    uint16_t ticks = epic_taskmgr_ticks();
    while (g_script_idx < SCRIPT_LEN && ticks >= SCRIPT[g_script_idx].tick)
    {
        if (SCRIPT[g_script_idx].kind == STIM_ADC)
        {
            control_demo_inject_adc(SCRIPT[g_script_idx].value);
        }
        else
        {
            control_demo_inject_console_byte((uint8_t)SCRIPT[g_script_idx].value);
        }
        g_fire_tick[g_script_idx] = ticks;
        g_script_idx++;
    }
}

/** @brief Log each injected entry's actual tick as 4 hex digits. */
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
 * @brief Run the scripted console/plant sequence and report PASS/FAIL.
 * @return 0 on pass, 1 on fail (see core/epic_harness.h)
 */
int main(void)
{
    epic_harness_init(SIM_ITERATIONS);
    control_demo_init();

    epic_taskmgr_init();
    epic_taskmgr_spawn(task_stimulus,             NULL, TASK_PERIOD_STIMULUS,  0U);
    epic_taskmgr_spawn(control_demo_task_control, NULL, TASK_PERIOD_CONTROL,   1U);
    epic_taskmgr_spawn(control_demo_task_console, NULL, TASK_PERIOD_CONSOLE,   2U);
    epic_taskmgr_spawn(control_demo_task_eeprom,  NULL, TASK_PERIOD_EEPROM,    3U);
    epic_taskmgr_spawn(control_demo_task_heartbeat, NULL, TASK_PERIOD_HEARTBEAT, 4U);

    epic_taskmgr_attach_timer0(TICK_RELOAD, TICK_PRESCALER);
    EPIC_IRQ_Restore(1);

    epic_taskmgr_run(); /* harness-bounded on the sim target */

    log_fire_ticks();

    CHECK(g_script_idx == SCRIPT_LEN, 0x00);          /* full script ran */
    CHECK(control_demo_setpoint() == 2800, 0x01);     /* set sp applied, 5000 rejected */
    CHECK(control_demo_gain_kp() == 64, 0x02);        /* set kp applied */
    CHECK(control_demo_gain_ki() == 4, 0x03);         /* set ki applied */
    CHECK(control_demo_gain_kd() == 32, 0x04);        /* set kd applied */
    CHECK(control_demo_measurement() == 2400U, 0x05); /* 600 raw x4 through both filters */
    CHECK(control_demo_output() >= 50, 0x06);         /* positive drive below setpoint */
    CHECK(control_demo_output() <= 1000, 0x07);       /* never past the clamp rail */
    CHECK(control_demo_eeprom_writes() >= 10U, 0x08); /* at least one full image save */
    CHECK((control_demo_eeprom_writes() % 10U) == 0U, 0x09); /* always whole 10-byte images */
    CHECK(epic_taskmgr_ticks() >= 100U, 0x0A);        /* tick source ran the whole budget */

    return epic_harness_report(g_fail == 0U);
}
