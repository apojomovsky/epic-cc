/**
 * Cooperative (non-preemptive) task scheduler for 8-bit PIC, family-
 * agnostic via `epic_hal.h`. A single periodic tick (typically Timer0)
 * marks due tasks ready; `epic_taskmgr_run_once()` runs them to
 * completion in priority order, no preemption, one shared call stack.
 */

#ifndef EPIC_TASKMGR_H
#define EPIC_TASKMGR_H

#include <stdint.h>
#include "peripherals/hal_timer0.h"   /* Timer0 tick source (family-neutral) */

/**
 * Maximum simultaneously registered tasks (~12 B/slot); scales via
 * EPIC_FAMILY_RAM_BYTES: 6 on 192 B parts, 8 otherwise. Override with
 * `#define EPIC_TASKMGR_MAX_TASKS` before including this header.
 */
#ifndef EPIC_TASKMGR_MAX_TASKS
#  if EPIC_FAMILY_RAM_BYTES <= 192
#    define EPIC_TASKMGR_MAX_TASKS  6
#  else
#    define EPIC_TASKMGR_MAX_TASKS  8
#  endif
#endif

/** Opaque task identifier returned by @ref epic_taskmgr_spawn. */
typedef uint8_t epic_taskmgr_id_t;

/** Sentinel for "no task" / an invalid spawn. */
#define EPIC_TASKMGR_ID_INVALID  ((epic_taskmgr_id_t)0xFFU)

/**
 * Task entry point: called with the `arg` passed to epic_taskmgr_spawn, runs to
 * completion and returns. Nothing else runs while it does; persist
 * per-task state through `arg`, not locals.
 */
typedef void (*epic_taskmgr_fn_t)(void *arg);

/** Task control block, one of EPIC_TASKMGR_MAX_TASKS fixed slots; all fields
 *  internal, tasks addressed by epic_taskmgr_id_t. */
typedef struct {
    epic_taskmgr_fn_t   fn;          /**< Entry point (NULL in a free slot). */
    void       *arg;
    uint16_t    period;      /**< Period in ticks; 0 = one-shot. */
    uint16_t    countdown;
    uint8_t     priority;    /**< Lower number = runs first within a round. */
    uint8_t     flags;       /**< Packed task flags. */
} epic_taskmgr_t;

/* Packed into epic_taskmgr_t.flags. */
#define EPIC_TASKMGR_FLAG_USED     0x01U   /**< Slot is allocated. */
#define EPIC_TASKMGR_FLAG_ENABLED  0x02U   /**< Slot is scheduled. */
#define EPIC_TASKMGR_FLAG_READY    0x04U   /**< Due this round. */

/**
 * @brief Initialise the scheduler.
 *
 * Clears every task slot and zeroes the tick counter. Call once before
 * spawning tasks or attaching a tick source. Idempotent.
 */
void epic_taskmgr_init(void);

/**
 * @brief Register a task and arm it.
 *
 * `fn` must not be NULL; returns EPIC_TASKMGR_ID_INVALID if it is, or when all
 * slots are in use.
 *
 * `period_ticks` 0 = one-shot (fires once, then frees its slot).
 * `priority`: lower runs first within a round; ties break by spawn order.
 * Safe to call from a running task (e.g. a supervisor spawning one-shot
 * children): the slot fill does not race with the tick ISR.
 *
 * @param fn            the task entry point; must not be NULL
 * @param arg           opaque pointer passed to `fn` on every run
 * @param period_ticks  firing period in ticks; 0 = one-shot
 * @param priority      lower runs first within a round
 * @return the new task's id, or EPIC_TASKMGR_ID_INVALID on failure
 */
epic_taskmgr_id_t epic_taskmgr_spawn(epic_taskmgr_fn_t fn, void *arg, uint16_t period_ticks,
                     uint8_t priority);

/**
 * @brief Enable a previously stopped task.
 *
 * Its countdown is reset to its period.
 *
 * @param id the task to start
 */
void epic_taskmgr_start(epic_taskmgr_id_t id);

/**
 * @brief Disable a task so the scheduler skips it until @ref epic_taskmgr_start.
 *
 * @param id the task to stop
 */
void epic_taskmgr_stop(epic_taskmgr_id_t id);

/**
 * @brief Change a task's period at runtime.
 *
 * Takes effect on the next arming.
 *
 * @param id            the task to retune
 * @param period_ticks  new period in ticks
 */
void epic_taskmgr_set_period(epic_taskmgr_id_t id, uint16_t period_ticks);

/**
 * @brief Re-arm a task from its full period.
 *
 * Restarts its countdown from the full period and ensures it is enabled,
 * without changing the period or freeing the slot (the debounce /
 * feed-the-watchdog verb). Safe to call from a running task.
 *
 * @param id the task to re-arm
 */
void epic_taskmgr_reset(epic_taskmgr_id_t id);

/**
 * @brief Advance the scheduler one tick.
 *
 * Decrements each enabled task's countdown, marking it ready (and
 * reloading periodic ones) at zero. Call from a timer ISR (typically
 * Timer0); it never runs user code, so it is safe in interrupt context.
 */
void epic_taskmgr_tick(void);

/**
 * @brief Current tick counter since epic_taskmgr_init.
 *
 * Wraps at 65535.
 *
 * @return the tick count
 */
uint16_t epic_taskmgr_ticks(void);

/**
 * @brief Run every task ready now, in priority order, then return.
 *        Not re-entrant: never call from inside a task (the snapshot
 *        array is a file-scope static, so a nested call would clobber
 *        the outer round's order).
 *
 * Non-blocking; call repeatedly from your main loop.
 *
 * @return number of tasks actually run this round
 */
uint8_t epic_taskmgr_run_once(void);

/**
 * @brief The canonical scheduler loop.
 *
 * Pumps the harness, runs due tasks, refreshes the watchdog. Bounded on
 * host (harness reports pass/fail and returns), runs forever on target.
 * Call epic_taskmgr_run_once directly if you need per-iteration work of
 * your own.
 */
void epic_taskmgr_run(void);

/**
 * @brief Number of tasks currently registered (used slots), any state.
 *
 * @return the count of used slots
 */
uint8_t epic_taskmgr_count(void);

/**
 * @brief Wire a HAL Timer0 overflow to epic_taskmgr_tick and start it.
 *
 * Call EPIC_IRQ_Restore(1) afterwards to actually arm it.
 *
 * @param reload     TMR0 reload value (0..255); on a 20 MHz target with
 *                   prescaler 1:256, reload 61 -> ~10 ms per tick
 * @param prescaler  a TIMER0_PrescalerTypeDef (1:2 .. 1:256)
 */
void epic_taskmgr_attach_timer0(uint8_t reload, TIMER0_PrescalerTypeDef prescaler);

#endif /* EPIC_TASKMGR_H */
