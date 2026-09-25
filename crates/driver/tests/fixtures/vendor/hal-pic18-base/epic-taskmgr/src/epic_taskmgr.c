/**
 * Cooperative task scheduler (see epic_taskmgr.h). The TCB array is the
 * only state shared with interrupt context: single-byte flag ops are
 * atomic, and the 16-bit TCB field writes (countdown/period) take a brief
 * critical section so a tick ISR never observes a torn value.
 */

#include "epic_taskmgr.h"
#include "core/hal_irq.h"   /* EPIC_IRQ_Disable/Restore (family-neutral) */
#include "core/epic_harness.h"      /* harness_tick / harness_running */
#include "core/hal_wdt_sleep.h"    /* EPIC_WDT_Refresh (family-neutral) */

/** The task table. Pinned to bank 2 (0x110, 64 bytes) on the 87XA: the
 *  TCBs are read from interrupt context and the 64-byte object needs one
 *  contiguous bank, so the placement is explicit rather than left to
 *  best-fit fragmentation. */
static epic_taskmgr_t g_tasks[EPIC_TASKMGR_MAX_TASKS] EPIC_PLACE(0x110);

/** Monotonic tick counter since the last init (wraps at 65535). */
static uint16_t g_ticks = 0U;

/**
 * @brief Countdown that fires a task after exactly `period` ticks.
 *
 * Periodic tasks reload to period-1, not period (else every fire would
 * take period+1 ticks); a one-shot (period 0) uses 0, ready on the first
 * tick.
 *
 * @param period the task's period in ticks
 * @return the countdown value to arm the task with
 */
static uint16_t arm_countdown(uint16_t period)
{
    return (period == 0U) ? 0U : (uint16_t)(period - 1U);
}

/** TMR0 reload value, rewritten each overflow since Timer0 has no
 *  hardware auto-reload; set by epic_taskmgr_attach_timer0. */
static uint8_t g_tick_reload = 0U;

/**
 * @brief Initialise the scheduler (see epic_taskmgr.h).
 *
 * Clears every task slot and zeroes the tick counter; idempotent.
 */
void epic_taskmgr_init(void)
{
    uint8_t prev = EPIC_IRQ_Disable();
    for (uint8_t i = 0; i < EPIC_TASKMGR_MAX_TASKS; i++)
    {
        g_tasks[i].fn        = NULL;
        g_tasks[i].arg       = NULL;
        g_tasks[i].period    = 0U;
        g_tasks[i].countdown = 0U;
        g_tasks[i].priority  = 0U;
        g_tasks[i].flags     = 0U;
    }
    g_ticks = 0U;
    EPIC_IRQ_Restore(prev);
}

/**
 * @brief Register a task and arm it (see epic_taskmgr.h).
 *
 * @param fn            the task entry point; must not be NULL
 * @param arg           opaque pointer passed to `fn` on every run
 * @param period_ticks  firing period in ticks; 0 = one-shot
 * @param priority      lower runs first within a round
 * @return the new task's id, or EPIC_TASKMGR_ID_INVALID on failure
 */
epic_taskmgr_id_t epic_taskmgr_spawn(epic_taskmgr_fn_t fn, void *arg, uint16_t period_ticks,
                     uint8_t priority)
{
    if (fn == NULL)
    {
        return EPIC_TASKMGR_ID_INVALID;
    }

    /* Publish-last: the TCB is fully written before the slot's USED bit
     * is set, and the tick ISR gates every access on USED. */
    epic_taskmgr_id_t id = EPIC_TASKMGR_ID_INVALID;
    for (uint8_t i = 0; i < EPIC_TASKMGR_MAX_TASKS; i++)
    {
        if (!(g_tasks[i].flags & EPIC_TASKMGR_FLAG_USED))
        {
            g_tasks[i].fn        = fn;
            g_tasks[i].arg       = arg;
            g_tasks[i].period    = period_ticks;
            g_tasks[i].countdown = arm_countdown(period_ticks);
            g_tasks[i].priority  = priority;
            g_tasks[i].flags     = EPIC_TASKMGR_FLAG_USED | EPIC_TASKMGR_FLAG_ENABLED;
            id = (epic_taskmgr_id_t)i;
            break;
        }
    }
    return id;
}

/**
 * @brief Enable a previously stopped task (see epic_taskmgr.h).
 *
 * @param id the task to start
 */
void epic_taskmgr_start(epic_taskmgr_id_t id)
{
    if (id >= EPIC_TASKMGR_MAX_TASKS) return;
    /* Critical section retained: countdown is a 16-bit store racing the
     * tick ISR's 16-bit countdown RMW, and retry cannot make a write
     * atomic; the ENABLED toggle alone leaves the ISR's check-then-act
     * window open. */
    uint8_t prev = EPIC_IRQ_Disable();
    if (g_tasks[id].flags & EPIC_TASKMGR_FLAG_USED)
    {
        g_tasks[id].countdown = arm_countdown(g_tasks[id].period);
        g_tasks[id].flags |=  EPIC_TASKMGR_FLAG_ENABLED;
        g_tasks[id].flags &= (uint8_t)~EPIC_TASKMGR_FLAG_READY;
    }
    EPIC_IRQ_Restore(prev);
}

/**
 * @brief Disable a task so the scheduler skips it until @ref epic_taskmgr_start (see epic_taskmgr.h).
 *
 * @param id the task to stop
 */
void epic_taskmgr_stop(epic_taskmgr_id_t id)
{
    if (id >= EPIC_TASKMGR_MAX_TASKS) return;
    /* Single-byte flag RMW, atomic on both families: no critical section. */
    if (g_tasks[id].flags & EPIC_TASKMGR_FLAG_USED)
    {
        g_tasks[id].flags &= (uint8_t)~(EPIC_TASKMGR_FLAG_ENABLED | EPIC_TASKMGR_FLAG_READY);
    }
}

/**
 * @brief Re-arm a task from its full period (see epic_taskmgr.h).
 *
 * @param id the task to re-arm
 */
void epic_taskmgr_reset(epic_taskmgr_id_t id)
{
    /* Same rationale as epic_taskmgr_start: the 16-bit countdown write cannot be
     * made atomic by retry. */
    if (id >= EPIC_TASKMGR_MAX_TASKS) return;
    uint8_t prev = EPIC_IRQ_Disable();
    if (g_tasks[id].flags & EPIC_TASKMGR_FLAG_USED)
    {
        g_tasks[id].countdown = arm_countdown(g_tasks[id].period);
        g_tasks[id].flags |=  EPIC_TASKMGR_FLAG_ENABLED;
        g_tasks[id].flags &= (uint8_t)~EPIC_TASKMGR_FLAG_READY;
    }
    EPIC_IRQ_Restore(prev);
}

/**
 * @brief Change a task's period at runtime (see epic_taskmgr.h).
 *
 * @param id            the task to retune
 * @param period_ticks  new period in ticks
 */
void epic_taskmgr_set_period(epic_taskmgr_id_t id, uint16_t period_ticks)
{
    if (id >= EPIC_TASKMGR_MAX_TASKS) return;
    /* Critical section retained: the 16-bit period write races the tick
     * ISR's period read when a task fires; a torn value mis-times one
     * fire. */
    uint8_t prev = EPIC_IRQ_Disable();
    if (g_tasks[id].flags & EPIC_TASKMGR_FLAG_USED)
    {
        g_tasks[id].period = period_ticks;
    }
    EPIC_IRQ_Restore(prev);
}

/**
 * @brief Advance the scheduler one tick (see epic_taskmgr.h).
 *
 * Safe in interrupt context; never runs user code.
 */
void epic_taskmgr_tick(void)
{
    g_ticks++;

    for (uint8_t i = 0; i < EPIC_TASKMGR_MAX_TASKS; i++)
    {
        epic_taskmgr_t *t = &g_tasks[i];
        uint8_t  f = t->flags;
        if (!(f & EPIC_TASKMGR_FLAG_USED) || !(f & EPIC_TASKMGR_FLAG_ENABLED))
        {
            continue;
        }
        if (t->countdown == 0U)
        {
            t->flags |= EPIC_TASKMGR_FLAG_READY;
            if (t->period != 0U)
            {
                t->countdown = arm_countdown(t->period);
            }
        }
        else
        {
            t->countdown--;
        }
    }
}

/**
 * @brief Current tick counter since epic_taskmgr_init (see epic_taskmgr.h).
 *
 * @return the tick count
 */
uint16_t epic_taskmgr_ticks(void)
{
    /* Read-twice-retry (the epic_tick_get pattern): the tick ISR
     * increments g_ticks as a 16-bit RMW, so a single read can tear;
     * retry until two consecutive reads agree. */
    uint16_t v;
    do
    {
        v = g_ticks;
    } while (v != g_ticks);
    return v;
}

/**
 * @brief Run every task ready now, in priority order (see epic_taskmgr.h).
 *
 * @return number of tasks actually run this round
 */
uint8_t epic_taskmgr_run_once(void)
{
    /* Snapshot the ready set in priority order, clearing each READY flag,
     * then run the tasks with interrupts enabled so a tick during a long
     * task arms the task for the next round. The snapshot array is a
     * file-scope static, not a stack local: a stack `order[]` alloca
     * is rejected by irparse on the epic-cc path (`[8 x i8]`
     * unsupported), and run_once is the main-loop dispatcher, never
     * re-entered, so the static is exclusive to it. */
    static epic_taskmgr_id_t s_order[EPIC_TASKMGR_MAX_TASKS];
    uint8_t n = 0U;

    /* No critical section: every flags access here is a single-byte read
     * or RMW (atomic), and the snapshot is timing-tolerant: a tick that
     * arms a task after its slot was scanned runs it next round. */
    for (;;)
    {
        /* Pick the lowest-numbered-priority ready task (ties: lowest slot). */
        int      best      = -1;
        uint8_t  best_prio = 0xFFU;
        for (uint8_t i = 0; i < EPIC_TASKMGR_MAX_TASKS; i++)
        {
            epic_taskmgr_t *t = &g_tasks[i];
            if ((t->flags & (EPIC_TASKMGR_FLAG_USED | EPIC_TASKMGR_FLAG_ENABLED | EPIC_TASKMGR_FLAG_READY))
                == (EPIC_TASKMGR_FLAG_USED | EPIC_TASKMGR_FLAG_ENABLED | EPIC_TASKMGR_FLAG_READY) &&
                t->priority < best_prio)
                {
                best      = (int)i;
                best_prio = t->priority;
            }
        }
        if (best < 0)
        {
            break;
        }
        g_tasks[best].flags &= (uint8_t)~EPIC_TASKMGR_FLAG_READY;  /* a new tick re-arms */
        s_order[n] = (epic_taskmgr_id_t)best;
        n++;
    }

    /* Run with interrupts enabled. */
    for (uint8_t k = 0; k < n; k++)
    {
        epic_taskmgr_t *t = &g_tasks[s_order[k]];
        t->fn(t->arg);
        if (t->period == 0U)
        {
            /* One-shot: free the slot so a periodic task re-spawning
             * one-shots cannot exhaust the table. Safe without a
             * critical section: the ISR skips slots with USED clear,
             * and a tick that read USED before the clear can only set
             * READY on a freed slot, which run_once gates out. */
            t->flags = 0U;
            t->fn    = NULL;
        }
    }
    return n;
}

/**
 * @brief The canonical scheduler loop (see epic_taskmgr.h).
 *
 * Bounded on host; runs forever on target.
 */
void epic_taskmgr_run(void)
{
    for (uint32_t i = 0; epic_harness_running(i); i++)
    {
        epic_harness_tick();    /* host: pumps sim → Timer0 ISR → tick */
        (void)epic_taskmgr_run_once();
        EPIC_WDT_Refresh();            /* no-op on the host */
    }
}

/**
 * @brief Number of tasks registered (used slots), any state (see epic_taskmgr.h).
 *
 * @return the count of used slots
 */
uint8_t epic_taskmgr_count(void)
{
    /* Single-byte flag reads (atomic), no critical section. A spawn/free
     * mid-scan shifts the count by one at worst: a timing artifact, not
     * a scheduler safety issue. */
    uint8_t count = 0U;
    for (uint8_t i = 0; i < EPIC_TASKMGR_MAX_TASKS; i++)
    {
        if (g_tasks[i].flags & EPIC_TASKMGR_FLAG_USED)
        {
            count++;
        }
    }
    return count;
}

/**
 * @brief Timer0 overflow callback.
 *
 * Reloads TMR0 first (no hardware auto-reload) so every tick has the
 * same period, then advances the scheduler.
 */
static void epic_taskmgr_on_timer0_overflow(void)
{
    EPIC_TIMER0_WriteCounter(g_tick_reload);
    epic_taskmgr_tick();
}

/**
 * @brief Wire a HAL Timer0 overflow to epic_taskmgr_tick and start it (see epic_taskmgr.h).
 *
 * @param reload     TMR0 reload value (0..255)
 * @param prescaler  a TIMER0_PrescalerTypeDef (1:2 .. 1:256)
 */
void epic_taskmgr_attach_timer0(uint8_t reload, TIMER0_PrescalerTypeDef prescaler)
{
    g_tick_reload = reload;
    TIMER0_HandleTypeDef h = TIMER0_HANDLE_DEFAULT;
    h.ClockSource       = TIMER0_CLOCK_INTERNAL;
    h.Prescaler         = prescaler;
    h.PrescalerAssigned = true;
    h.ReloadValue       = reload;
    h.OverflowCallback  = epic_taskmgr_on_timer0_overflow;
    EPIC_TIMER0_Init(&h);
    EPIC_TIMER0_Start(&h);
}
