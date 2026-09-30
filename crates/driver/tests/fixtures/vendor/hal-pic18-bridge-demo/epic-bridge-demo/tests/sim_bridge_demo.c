/**
 * @brief Bounded, self-reporting mdb sim gate for epic-bridge-demo.
 *
 * Runs the exact bridge_demo_core.c the real target uses, but MPLAB
 * SIM delivers neither UART RX bytes nor I2C transactions, so a
 * scheduled taskmgr task feeds both deterministically: request frames
 * go into bridge_demo_inject_rx_byte one whole frame per dispatch
 * (paced on bridge_demo_staged_idle, never on wall time), ADC readings
 * into bridge_demo_inject_adc, and the expander uses the core's
 * scripted register file. Asserts cover byte-exact responses, the
 * exception frame, the two silent drops, the map images, and the
 * expander shadow, plus a GIE-masked wire capture of one response.
 */

#include "bridge_demo_core.h"

#include "core/epic_harness.h"
#include "epic_hal.h"
#include "epic_serial.h"
#include "epic_taskmgr.h"
#include "epic_tick.h"

#include <stdint.h>
/** Loop-iteration bound for epic_taskmgr_run()'s harness-driven loop
 * (core/epic_harness.h: not a real-time unit). Idle spins make one
 * iteration ~1/6 of a 341 us tick (measured 150 iters to 24 ticks),
 * and each scripted frame needs one 5 ms T3.5 silence (~15 ticks),
 * so seven frames need ~110 ticks; 900 leaves measured margin in
 * the 60 s mdb budget. */
#define SIM_ITERATIONS 900UL
#define TASK_PERIOD_STIMULUS  1U
#define TICK_RELOAD    0U
#define TICK_PRESCALER TIMER0_PRESCALER_1_16

#define TASK_PERIOD_BRIDGE 1U
#define TASK_PERIOD_ADC    5U

#define ADC0_SCRIPT 600U
#define ADC1_SCRIPT 300U
#define AVG_X4(v)   ((uint16_t)((v) * 4U))

/** One scripted request frame: threshold plus the six payload bytes.
 * The CRC is computed at startup by build_frames, so the gate carries
 * an independent CRC oracle like the modbus module's own sim gate. */
typedef struct {
    uint16_t tick;
    uint8_t addr;
    uint8_t fc;
    uint8_t p_hi;
    uint8_t p_lo;
    uint8_t q_hi;
    uint8_t q_lo;
    uint8_t crc_bad;
} frame_spec_t;

static const frame_spec_t FRAMES[] = {
    { 5U,  0x01U, 0x06U, 0x00U, 0x00U, 0x00U, 0xA5U, 0U },
    { 10U, 0x01U, 0x06U, 0x00U, 0x01U, 0x00U, 0x5AU, 0U },
    { 15U, 0x01U, 0x03U, 0x00U, 0x00U, 0x00U, 0x06U, 0U },
    { 20U, 0x01U, 0x04U, 0x00U, 0x00U, 0x00U, 0x04U, 0U },
    { 25U, 0x01U, 0x03U, 0x01U, 0x00U, 0x00U, 0x01U, 0U },
    { 30U, 0x01U, 0x03U, 0x00U, 0x00U, 0x00U, 0x01U, 1U },
    { 35U, 0x02U, 0x03U, 0x00U, 0x00U, 0x00U, 0x01U, 0U },
};
#define FRAME_LEN (sizeof(FRAMES) / sizeof(FRAMES[0]))
#define FRAME_SZ  8U

static uint8_t g_frames[FRAME_LEN][FRAME_SZ];
static uint8_t g_frame_idx;
static uint8_t g_adc_done;
static uint16_t g_fire_tick[FRAME_LEN];

/** @brief Independent CRC-16/Modbus reference (see sim_modbus.c). */
static uint16_t ref_crc16(const uint8_t *buf, uint8_t len)
{
    uint16_t crc = 0xFFFFU;
    uint8_t i;
    uint8_t bit;
    for (i = 0U; i < len; i++)
    {
        crc = (uint16_t)(crc ^ buf[i]);
        for (bit = 0U; bit < 8U; bit++)
        {
            if (crc & 1U)
            {
                crc = (uint16_t)((crc >> 1) ^ 0xA001U);
            }
            else
            {
                crc = (uint16_t)(crc >> 1);
            }
        }
    }
    return crc;
}

/** @brief Assemble every scripted frame plus its (possibly bad) CRC. */
static void build_frames(void)
{
    uint8_t f;
    uint16_t crc;
    for (f = 0U; f < FRAME_LEN; f++)
    {
        g_frames[f][0] = FRAMES[f].addr;
        g_frames[f][1] = FRAMES[f].fc;
        g_frames[f][2] = FRAMES[f].p_hi;
        g_frames[f][3] = FRAMES[f].p_lo;
        g_frames[f][4] = FRAMES[f].q_hi;
        g_frames[f][5] = FRAMES[f].q_lo;
        crc = ref_crc16(g_frames[f], 6U);
        g_frames[f][6] = (uint8_t)(crc & 0xFFU);
        g_frames[f][7] = (uint8_t)(crc >> 8);
        if (FRAMES[f].crc_bad)
        {
            g_frames[f][6] = (uint8_t)(g_frames[f][6] ^ 0xFFU);
        }
    }
}

/** @brief Inject due ADC values plus at most one frame per dispatch. */
static void task_stimulus(void *arg)
{
    uint8_t i;
    (void)arg;
    if (g_adc_done == 0U && epic_taskmgr_ticks() >= 2U)
    {
        bridge_demo_inject_adc(0U, ADC0_SCRIPT);
        bridge_demo_inject_adc(1U, ADC1_SCRIPT);
        g_adc_done = 1U;
    }
    if (g_frame_idx >= FRAME_LEN)
    {
        return;
    }
    if (epic_taskmgr_ticks() < FRAMES[g_frame_idx].tick)
    {
        return;
    }
    if (bridge_demo_staged_idle() == 0U)
    {
        return;
    }
    for (i = 0U; i < FRAME_SZ; i++)
    {
        bridge_demo_inject_rx_byte(g_frames[g_frame_idx][i]);
    }
    g_fire_tick[g_frame_idx] = epic_taskmgr_ticks();
    g_frame_idx++;
}

/** @brief Log each injected frame's actual tick as 4 hex digits. */
static void log_fire_ticks(void)
{
    static const char hx[] = "0123456789ABCDEF";
    static char buf[6];
    uint8_t i;
    for (i = 0U; i < g_frame_idx; i++)
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

/** @brief Record a check failure and log its index as two hex digits. */
static void fail(uint8_t idx)
{
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

/** @brief Compare two byte buffers for equality. */
static uint8_t bufeq(const uint8_t *a, const uint8_t *b, uint8_t n)
{
    uint8_t i;
    for (i = 0U; i < n; i++)
    {
        if (a[i] != b[i])
        {
            return 0U;
        }
    }
    return 1U;
}

/** @brief Check a logged response's trailing CRC against a reference. */
static uint8_t crcok(const uint8_t *buf, uint8_t len)
{
    uint16_t crc;
    if (len < 3U)
    {
        return 0U;
    }
    crc = ref_crc16(buf, (uint8_t)(len - 2U));
    return (uint8_t)(buf[(uint8_t)(len - 2U)] == (uint8_t)(crc & 0xFFU) &&
                     buf[(uint8_t)(len - 1U)] == (uint8_t)(crc >> 8));
}

static uint8_t g_buf[20];

/** @brief Fetch logged response idx and check its exact length. */
static uint8_t get_resp(uint8_t idx, uint8_t want_len)
{
    uint8_t len = bridge_demo_response(idx, g_buf, (uint8_t)sizeof(g_buf));
    return (uint8_t)(len == want_len);
}

/** @brief GIE-masked wire capture of one fresh read-holding response. */
static uint8_t g_wire[20];
static uint8_t g_wire_n;

/** @brief Inject a read, dispatch with the bridge task, capture TXREG. */
static void wire_probe(void)
{
    uint8_t prev;
    uint8_t req[8];
    uint8_t i;
    uint32_t guard;
    uint32_t t0;
    req[0] = 0x01U;
    req[1] = 0x03U;
    req[2] = 0x00U;
    req[3] = 0x00U;
    req[4] = 0x00U;
    req[5] = 0x06U;
    {
        uint16_t crc = ref_crc16(req, 6U);
        req[6] = (uint8_t)(crc & 0xFFU);
        req[7] = (uint8_t)(crc >> 8);
    }
    prev = EPIC_IRQ_Disable();
    guard = 0UL;
    while (bridge_demo_staged_idle() == 0U && guard < 20000000UL)
    {
        guard++;
    }
    if (guard >= 20000000UL)
    {
        EPIC_IRQ_Restore(prev);
        return;
    }
    /* Flush script traffic already proven by the log checks: at 9600
     * baud the burst outruns the shift register, so stale response
     * bytes may still sit in the ring. Discard them with the same
     * TX-only servicing the capture uses, so the probe captures
     * exactly its own 17 bytes below. */
    guard = 0UL;
    while (epic_serial_tx_pending() > 0 && guard < 4000000UL)
    {
        guard++;
        if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TXIF)
        {
            USART_TX_IRQHandler();
        }
    }
    if (guard >= 4000000UL)
    {
        EPIC_IRQ_Restore(prev);
        return;
    }
    for (i = 0U; i < 8U; i++)
    {
        bridge_demo_inject_rx_byte(req[i]);
    }
    /* GIE stays off so the taskmgr tick (and the map's tick regs)
     * cannot move under the capture, which also freezes the tick
     * counter the silence check reads. Poll the Timer2 match flag
     * and run its handler by hand, the same call the dispatcher
     * would make, until the 5 ms frame silence (T35_MS in the core)
     * elapses in the counter. */
    t0 = epic_tick_get();
    guard = 0UL;
    while (epic_tick_elapsed_since(t0) < 5U && guard < 4000000UL)
    {
        guard++;
        if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TMR2IF)
        {
            TIMER2_IRQHandler();
        }
    }
    if (guard >= 4000000UL)
    {
        EPIC_IRQ_Restore(prev);
        return;
    }
    bridge_demo_task_bridge(NULL);
    guard = 0UL;
    while (epic_serial_tx_pending() > 0 && g_wire_n < (uint8_t)sizeof(g_wire) &&
           guard < 1000000UL)
    {
        guard++;
        /* TX only, not the full dispatcher: hand-servicing foreign
         * sources while GIE is masked risks their SIM reentrancy
         * quirks, and nothing here needs them. The TX handler just
         * pops one ring byte into TXREG, the same call the
         * dispatcher would make for this flag. */
        if (EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_TXIF)
        {
            USART_TX_IRQHandler();
            g_wire[g_wire_n] = EPIC_REG8(PIC_REG_TXREG);
            g_wire_n++;
        }
    }
}

/** @brief Run the scripted bridge sequence and report PASS/FAIL. */
int main(void)
{
    uint8_t exp5[5];
    uint16_t crc;
    epic_harness_init(SIM_ITERATIONS);
    build_frames();
    bridge_demo_init();
    bridge_demo_use_sim_expander();

    epic_taskmgr_init();
    epic_taskmgr_spawn(task_stimulus,         NULL, TASK_PERIOD_STIMULUS, 0U);
    epic_taskmgr_spawn(bridge_demo_task_bridge, NULL, TASK_PERIOD_BRIDGE, 1U);
    epic_taskmgr_spawn(bridge_demo_task_adc,  NULL, TASK_PERIOD_ADC,      2U);

    epic_taskmgr_attach_timer0(TICK_RELOAD, TICK_PRESCALER);
    EPIC_IRQ_Restore(1);

    epic_taskmgr_run(); /* harness-bounded on the sim target */

    CHECK(g_frame_idx == FRAME_LEN, 0x00);      /* full script ran */
    CHECK(bridge_demo_response_count() == 5U, 0x01); /* 2 silent drops */
    CHECK(get_resp(0U, 8U) && bufeq(g_buf, g_frames[0], 8U), 0x02);
    CHECK(get_resp(1U, 8U) && bufeq(g_buf, g_frames[1], 8U), 0x03);
    CHECK(bridge_demo_holding(0U) == 0xA5U &&
          bridge_demo_holding(1U) == 0x5AU, 0x04); /* writes applied */
    CHECK(bridge_demo_expander_shadow() == 0x5AA5U, 0x05); /* GPIO followed */

    /* F3 answered from converged state: fixed regs exact, tick bytes
     * prove nothing (still running then), CRC proves the whole frame. */
    CHECK(get_resp(2U, 17U) && g_buf[0] == 0x01U && g_buf[1] == 0x03U &&
          g_buf[2] == 0x0CU && g_buf[3] == 0x00U && g_buf[4] == 0xA5U &&
          g_buf[5] == 0x00U && g_buf[6] == 0x5AU &&
          g_buf[7] == (uint8_t)(AVG_X4(ADC0_SCRIPT) >> 8) &&
          g_buf[8] == (uint8_t)(AVG_X4(ADC0_SCRIPT) & 0xFFU) &&
          g_buf[9] == (uint8_t)(AVG_X4(ADC1_SCRIPT) >> 8) &&
          g_buf[10] == (uint8_t)(AVG_X4(ADC1_SCRIPT) & 0xFFU) &&
          g_buf[13] == 0x00U && g_buf[14] == 0x00U && crcok(g_buf, 17U), 0x06);
    CHECK(get_resp(3U, 13U) && g_buf[0] == 0x01U && g_buf[1] == 0x04U &&
          g_buf[2] == 0x08U &&
          g_buf[3] == (uint8_t)(AVG_X4(ADC0_SCRIPT) >> 8) &&
          g_buf[4] == (uint8_t)(AVG_X4(ADC0_SCRIPT) & 0xFFU) &&
          g_buf[5] == (uint8_t)(AVG_X4(ADC1_SCRIPT) >> 8) &&
          g_buf[6] == (uint8_t)(AVG_X4(ADC1_SCRIPT) & 0xFFU) &&
          g_buf[9] == 0x00U && g_buf[10] == 0x00U && crcok(g_buf, 13U), 0x07);

    exp5[0] = 0x01U;
    exp5[1] = 0x83U;
    exp5[2] = 0x02U;
    crc = ref_crc16(exp5, 3U);
    exp5[3] = (uint8_t)(crc & 0xFFU);
    exp5[4] = (uint8_t)(crc >> 8);
    CHECK(get_resp(4U, 5U) && bufeq(g_buf, exp5, 5U), 0x08);
    CHECK(bridge_demo_last_exception() == 0x02U, 0x09);
    CHECK(bridge_demo_tx_frames() == 5U, 0x0A);
    CHECK(bridge_demo_holding(2U) == AVG_X4(ADC0_SCRIPT) &&
          bridge_demo_holding(3U) == AVG_X4(ADC1_SCRIPT), 0x0B);
    CHECK(bridge_demo_input(0U) == AVG_X4(ADC0_SCRIPT) &&
          bridge_demo_input(1U) == AVG_X4(ADC1_SCRIPT), 0x0C);
    CHECK(bridge_demo_holding(5U) == 0x0002U &&
          bridge_demo_input(3U) == 0x0002U, 0x0D); /* exc, no I2C error */
    CHECK(epic_taskmgr_ticks() >= 100U, 0x0E); /* tick source ran */

    /* Wire proof: the read answered now must match the logged response
     * byte for byte at TXREG. Compared against the log, not a rebuilt
     * image: the holding tick moves under manual IRQ dispatch in SIM,
     * so a rebuild races the capture while the log is immutable once
     * staged_send runs. Content is already proven by the checks above. */
    wire_probe();
    CHECK(get_resp(5U, 17U) && g_wire_n == 17U && bufeq(g_wire, g_buf, 17U), 0x0F);
    CHECK(epic_serial_tx_pending() == 0, 0x10); /* ring fully drained */
    CHECK(bridge_demo_tx_frames() == 6U, 0x11); /* probe logged too */

    return epic_harness_report(g_fail == 0U);
}
