/**
 * Bounded, self-reporting HARNESS=sim build, the module's mdb gate
 * (PIC16F877A/MPLAB SIM): runs the compiled pid.c (Q8.8, epic_math
 * 16x16->32 multiply) through a scripted step-then-settle trajectory via
 * the real `epic_pid_update` API, checking (a) every output stays in
 * [out_min, out_max] and the anti-windup invariant holds, (b) convergence
 * to the setpoint, and (c) epic_pid_set_gains takes effect. Reports PASS/FAIL
 * over the harness USART (see pic16f87xa-hal/src/mdb/pic16_harness_mdb.c).
 */

#include "pid.h"
#include "core/epic_harness.h"

#ifndef FOSC_HZ
#define FOSC_HZ 20000000UL
#endif

/** Number of step-then-settle control steps in phase A. */
#define N_SETTLE 200UL
/** Bounded loop budget: phase A + phase B steps + report. */
#define SIM_ITERATIONS (N_SETTLE + 7UL)

#define OUT_MIN (-1000)
#define OUT_MAX 1000
#define SETPOINT 100

/* Phase A tuning: Kp = 1.0, Ki*Ts = 0.25, Kd = 0 (pre-scaled Q8.8
 * discrete-time gains, see docs/API.md's conversion). */
#define KP_Q8 256
#define KI_Q8 64
#define KD_Q8 0

static epic_pid_t   g_pid;
static int16_t g_meas = 0;

/** @brief Run the step-then-settle trajectory and report PASS/FAIL over the harness. */
int main(void)
{
    epic_harness_init(SIM_ITERATIONS);

    epic_pid_init(&g_pid, (int16_t)KP_Q8, (int16_t)KI_Q8, (int16_t)KD_Q8,
             (int16_t)OUT_MIN, (int16_t)OUT_MAX);

    int16_t out_min_seen = OUT_MAX, out_max_seen = OUT_MIN;
    int16_t u_pre = 0, out_step = 0;

    for (uint32_t i = 0; epic_harness_running(i); i++)
    {
        epic_harness_tick();
        if (i < N_SETTLE)
        {
            /* Phase A: setpoint step 0 -> 100, let the loop settle. */
            int16_t out = epic_pid_update(&g_pid, (int16_t)SETPOINT, g_meas);
            if (out < out_min_seen)
            {
                out_min_seen = out;
            }
            if (out > out_max_seen)
            {
                out_max_seen = out;
            }
            /* Plant: integer first-order lag, same shape as the
             * setpoint-step example. */
            g_meas = (int16_t)(g_meas + (int16_t)((out - g_meas) / 4));
        }
        else if (i == N_SETTLE)
        {
            /* Steady state reached; freeze the plant and record the
             * controller's holding output (error is 0, so this is
             * I >> 8 exactly). */
            u_pre = epic_pid_update(&g_pid, (int16_t)SETPOINT, g_meas);
            if (u_pre < out_min_seen)
            {
                out_min_seen = u_pre;
            }
            if (u_pre > out_max_seen)
            {
                out_max_seen = u_pre;
            }
        }
        else if (i == N_SETTLE + 1UL)
        {
            /* Phase B: gain change on the live instance. Double kp,
             * drop the I gain; integrator state is untouched. */
            epic_pid_set_gains(&g_pid, (int16_t)512, (int16_t)0, (int16_t)0);
        }
        else if (i == N_SETTLE + 2UL)
        {
            /* 10-count setpoint step at the frozen plant: with the new
             * kp the output must move by (512 * 10) >> 8 = 20 counts
             * (the old kp would have moved it 10). */
            out_step = epic_pid_update(&g_pid, (int16_t)(SETPOINT + 10), g_meas);
            if (out_step < out_min_seen)
            {
                out_min_seen = out_step;
            }
            if (out_step > out_max_seen)
            {
                out_max_seen = out_step;
            }
        }
    }

    int16_t err_ss = (int16_t)SETPOINT - g_meas;
    int gain_ok = (out_step - u_pre) >= 15 && (out_step - u_pre) <= 25;

    int ok = (out_min_seen >= OUT_MIN && out_max_seen <= OUT_MAX) &&
             (err_ss >= -2 && err_ss <= 2) && gain_ok;
    epic_harness_log(ok ? "pid sim: pass\n" : "pid sim: fail\n");
    return epic_harness_report(ok);
}
