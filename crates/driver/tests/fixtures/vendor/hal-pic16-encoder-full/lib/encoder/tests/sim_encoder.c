/**
 * Bounded, self-reporting HARNESS=sim build, the module's mdb gate
 * (PIC16F877A/MPLAB SIM): feeds a scripted quadrature sequence through
 * `epic_encoder_update` verifying the x4 position count in both directions,
 * the impossible-transition counter, and the glitch gate against real
 * simulated time, then hammers `epic_encoder_get_position` under the live
 * tick ISR, checking every read is consistent and the tick survived.
 * Reports PASS/FAIL over the harness
 * USART (see hal/pic14/16f87xa/src/mdb/pic16_harness_mdb.c).
 */

#include "encoder.h"
#include "epic_tick.h"
#include "core/epic_harness.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 20000000UL
#endif

#define PIN_A 4u
#define PIN_B 5u

#define HAMMER_READS      5000UL
#define HAMMER_WAIT_EVERY 1000UL
/** Bound for a 1 ms tick wait (~5000 instructions when healthy); only
 *  the class-G failure path (dead tick) ever gets near it. */
#define WAIT_SPIN_BOUND   100000UL
/** Harness iteration budget: the sim target's epic_harness_running is
 *  `iteration < cycles` (0 would make every harness loop a no-op), so
 *  this must exceed HAMMER_READS. */
#define SIM_ITERATIONS    100000UL

static epic_encoder_t g_dec;    /* phase 1+2: decode + impossible transition */
static epic_encoder_t g_gate;   /* phase 3:   glitch gate against real time   */
static epic_encoder_t g_ham;    /* phase 4:   class-G read hammer             */

/**
 * @brief Build a port byte putting the 2-bit state (a<<1|b) at pins PIN_A/PIN_B.
 */
static uint8_t port_byte(uint8_t state)
{
    uint8_t v = 0U;
    if (state & 0x2U) v |= (uint8_t)(1U << PIN_A);
    if (state & 0x1U) v |= (uint8_t)(1U << PIN_B);
    return v;
}

/**
 * @brief Wait for the tick counter to advance 1 ms, bounded.
 *
 * Returns 1 if the tick advanced (GIE intact), 0 if the tick stalled
 * (class-G GIE loss).
 */
static uint8_t wait_1ms_bounded(void)
{
    uint32_t t0 = epic_tick_get();
    uint32_t spins = 0u;
    while (epic_tick_elapsed_since(t0) < 1u)
    {
        epic_harness_tick();
        if (++spins >= WAIT_SPIN_BOUND)
        {
            return 0u;
        }
    }
    return 1u;
}

/* epic-cc folds a single-callsite helper into its caller; keep the
 * attribute so main stays inside one page. */
/**
 * @brief Hammer the class-G position read under the live tick ISR, returning tear/stall/tick_ok packed in bits 0/1/2.
 */
static uint8_t __attribute__((noinline)) phase4_hammer(void)
{
    /* Position is never written here, so any non-zero read is a torn
     * read; a wait that spins out its budget means the tick stopped
     * (GIE left cleared). Packed so the caller keeps the same checks
     * without the loop inline. */
    epic_encoder_init(&g_ham, PIN_A, PIN_B, 0u, port_byte(0u));
    uint32_t t_start = epic_tick_get();
    int tear = 0;
    int stall = 0;
    for (uint32_t i = 0; epic_harness_running(i) && i < HAMMER_READS; i++)
    {
        epic_harness_tick();
        if (epic_encoder_get_position(&g_ham) != 0)
        {
            tear = 1;
        }
        if ((i % HAMMER_WAIT_EVERY) == 0u && !wait_1ms_bounded())
        {
            stall = 1;
        }
    }
    uint32_t t_end = epic_tick_get();
    int tick_ok = (t_end > t_start) && !stall;
    return (uint8_t)((uint8_t)tear | (uint8_t)(stall << 1) | (uint8_t)(tick_ok << 2));
}

/** @brief Run the scripted decode/glitch/class-G sequence and report PASS/FAIL. */
int main(void)
{
    epic_harness_init(SIM_ITERATIONS);
    epic_tick_init(FOSC_HZ);

    /* ---- Phase 1: two full rotations each way, gate off (host test
     *      sequence verbatim). -8 then +8 -> back to 0, no errors. */
    static const uint8_t neg_seq[8] = { 1, 3, 2, 0, 1, 3, 2, 0 };
    static const uint8_t pos_seq[8] = { 2, 3, 1, 0, 2, 3, 1, 0 };
    epic_encoder_init(&g_dec, PIN_A, PIN_B, 0u, port_byte(0u));
    for (uint8_t i = 0; i < 8u; i++)
    {
        epic_encoder_update(&g_dec, port_byte(neg_seq[i]));
    }
    for (uint8_t i = 0; i < 8u; i++)
    {
        epic_encoder_update(&g_dec, port_byte(pos_seq[i]));
    }
    int32_t p_dec   = epic_encoder_get_position(&g_dec);
    uint16_t e_dec  = epic_encoder_get_error_count(&g_dec);
    uint16_t g_dec0 = epic_encoder_get_glitch_count(&g_dec);

    /* ---- Phase 2: impossible 00->11 (both bits flip), then resync. */
    epic_encoder_update(&g_dec, port_byte(3u));   /* 00->11: error, no count */
    int32_t p_imp = epic_encoder_get_position(&g_dec);
    uint16_t e_imp = epic_encoder_get_error_count(&g_dec);
    epic_encoder_update(&g_dec, port_byte(1u));   /* 11->01: valid +1 edge  */
    int32_t p_resync = epic_encoder_get_position(&g_dec);

    /* ---- Phase 3: glitch gate on real tick time (host test script). */
    epic_encoder_init(&g_gate, PIN_A, PIN_B, 10u, port_byte(0u));
    for (uint8_t i = 0; i < 10u; i++) wait_1ms_bounded();  /* clear seed gate */
    epic_encoder_update(&g_gate, port_byte(1u));                /* 00->01 accepted */
    int32_t pg_first = epic_encoder_get_position(&g_gate);
    wait_1ms_bounded();
    wait_1ms_bounded();                                    /* +2 ms          */
    epic_encoder_update(&g_gate, port_byte(3u));                /* 01->11 too soon */
    uint16_t gg_drop = epic_encoder_get_glitch_count(&g_gate);
    int32_t pg_drop  = epic_encoder_get_position(&g_gate);
    for (uint8_t i = 0; i < 12u; i++) wait_1ms_bounded();  /* +12 ms         */
    epic_encoder_update(&g_gate, port_byte(3u));                /* 01->11 accepted */
    int32_t pg_accept = epic_encoder_get_position(&g_gate);
    uint16_t gg_after = epic_encoder_get_glitch_count(&g_gate);
    uint16_t eg_gate  = epic_encoder_get_error_count(&g_gate);

    /* ---- Phase 4: class-G probe (hammer loop lives in phase4_hammer). */
    uint8_t ham = phase4_hammer();
    int tear = (int)(ham & 1u);
    int tick_ok = (int)((ham >> 2) & 1u);

    /* ---- Report. ---- */
    int phase1_ok = (p_dec == 0) && (e_dec == 0) && (g_dec0 == 0);
    int phase2_ok = (p_imp == 0) && (e_imp == 1) && (p_resync == 1);
    int phase3_ok = (pg_first == -1) && (gg_drop == 1) && (pg_drop == -1) &&
                    (pg_accept == -2) && (gg_after == 1) && (eg_gate == 0);

    if (phase1_ok)
    {
        epic_harness_log("encoder sim: rotations ok (-8 then +8, no errors)\n");
    }
    else
    {
        epic_harness_log("encoder sim: rotation decode MISMATCH\n");
    }
    if (phase2_ok)
    {
        epic_harness_log("encoder sim: impossible transition + resync ok\n");
    }
    else
    {
        epic_harness_log("encoder sim: impossible transition MISMATCH\n");
    }
    if (phase3_ok)
    {
        epic_harness_log("encoder sim: glitch gate ok (window respected)\n");
    }
    else
    {
        epic_harness_log("encoder sim: glitch gate MISMATCH\n");
    }
    if (tear)
    {
        epic_harness_log("encoder sim: class-G probe saw a TORN read\n");
    }
    if (tick_ok)
    {
        epic_harness_log("encoder sim: class-G probe: tick survived\n");
    }
    else
    {
        epic_harness_log("encoder sim: class-G probe: tick STALLED (GIE lost)\n");
    }

    int ok = phase1_ok && phase2_ok && phase3_ok && !tear && tick_ok;
    return epic_harness_report(ok);
}
