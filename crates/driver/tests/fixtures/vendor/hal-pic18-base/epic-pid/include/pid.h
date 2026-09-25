/**
 * Vendor-agnostic, single-loop, fixed-point (Q8.8) PID controller with
 * anti-windup, derivative-on-measurement, and bumpless auto/manual
 * transfer. Pure arithmetic, no HAL dependency; gains are pre-scaled by
 * Ts so epic_pid_update needs no division. See docs/API.md for the gain
 * conversion.
 */

#ifndef PID_H
#define PID_H

#include <stdint.h>
#include <stdbool.h>

/** Auto / manual mode selector (see @ref epic_pid_set_mode, @ref epic_pid_update). */
typedef enum {
    EPIC_PID_MODE_MANUAL = 0,
    EPIC_PID_MODE_AUTO,
} epic_pid_mode_t;

/**
 * One PID control loop, caller-owned storage. Fields are written by
 * epic_pid_init / epic_pid_set_* and by epic_pid_update; the caller reads them through
 * the API.
 */
typedef struct {
    int16_t    kp_q8, ki_q8, kd_q8;   /* Q8.8 gains; ki_q8 includes *Ts, kd_q8 includes /Ts */
    int16_t    out_min, out_max;      /* actuator output clamp; out_min <= out_max */

    int32_t    integrator_q8;         /* Q8.8 integral term, clamped to [out_min,out_max]<<8 */
    int16_t    prev_measurement;      /* for derivative-on-measurement */
    bool       have_prev_measurement; /* false until first epic_pid_update() since init/reset;
                                        * gates the D term to avoid a first-call kick */
    bool       skip_next_i_increment; /* set after a MANUAL call back-calculates the
                                        * integrator; the next AUTO call skips the I
                                        * increment (bumpless transfer), then clears this */
    epic_pid_mode_t mode;
    int16_t    manual_output;         /* caller-set target output while mode == MANUAL */
} epic_pid_t;

/**
 * @brief Initialize a PID instance.
 *
 * Stores gains and clamp range, sets AUTO mode, zeroes the integrator
 * and D-term history.
 *
 * @param pid       the controller instance to initialize
 * @param kp_q8     Q8.8 proportional gain (= round(Kp * 256))
 * @param ki_q8     Q8.8 integral gain, pre-multiplied by Ts (= round(Ki * Ts * 256))
 * @param kd_q8     Q8.8 derivative gain, pre-divided by Ts (= round(Kd / Ts * 256))
 * @param out_min   lower actuator clamp rail (out_min <= out_max)
 * @param out_max   upper actuator clamp rail
 */
void epic_pid_init(epic_pid_t *pid, int16_t kp_q8, int16_t ki_q8, int16_t kd_q8,
              int16_t out_min, int16_t out_max);

/**
 * @brief  Zero the integrator and clear the D-term history, without losing
 *         tuning (gains/clamp/mode untouched), for recovering from an
 *         external fault (e-stop, sensor dropout).
 *
 * @param pid the controller instance to reset
 */
void epic_pid_reset(epic_pid_t *pid);

/**
 * @brief  Replace the three gains, leaving the integrator, D-term history,
 *         and mode untouched.
 *
 * @param pid   the controller instance to retune
 * @param kp_q8 Q8.8 proportional gain (= round(Kp * 256))
 * @param ki_q8 Q8.8 integral gain, pre-multiplied by Ts (= round(Ki * Ts * 256))
 * @param kd_q8 Q8.8 derivative gain, pre-divided by Ts (= round(Kd / Ts * 256))
 */
void epic_pid_set_gains(epic_pid_t *pid, int16_t kp_q8, int16_t ki_q8, int16_t kd_q8);

/**
 * @brief  Switch between AUTO and MANUAL.
 *
 * Does NOT reset the integrator or D-term history; switching mode is not
 * a fault, and bumpless transfer depends on integrator state carrying
 * across the switch.
 *
 * @param pid   the controller instance to switch
 * @param mode  the new mode (EPIC_PID_MODE_AUTO or EPIC_PID_MODE_MANUAL)
 */
void epic_pid_set_mode(epic_pid_t *pid, epic_pid_mode_t mode);

/**
 * @brief Set the target output used while mode == EPIC_PID_MODE_MANUAL.
 *
 * Only consulted by epic_pid_update() in MANUAL, ignored in AUTO. Call every
 * cycle the operator wants a new manual output in effect.
 *
 * @param pid    the controller instance to drive
 * @param value  the manual output target
 */
void epic_pid_set_manual_output(epic_pid_t *pid, int16_t value);

/**
 * @brief Step the controller once per fixed control-loop period.
 *
 * The single per-cycle entry point. AUTO: `clamp((P+I+D) >> 8,
 * out_min, out_max)` with D from `-d(measurement)/dt` and the
 * integrator clamped to `[out_min, out_max] << 8` (anti-windup).
 * MANUAL: `clamp(manual_output, out_min, out_max)`, back-calculating
 * the integrator so resuming AUTO is bumpless. Precondition (not
 * runtime-checked): `|setpoint - measurement| <= 32767`.
 *
 * @param pid          the controller instance to step
 * @param setpoint     the target value
 * @param measurement  the measured process value
 *
 * @return the clamped output, always in `[out_min, out_max]`
 */
int16_t epic_pid_update(epic_pid_t *pid, int16_t setpoint, int16_t measurement);

#endif /* PID_H */
