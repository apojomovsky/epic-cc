/**
 * Real-target implementation of the harness: no-ops, since the CPU
 * starts itself, time advances on its own, and there is no stdout.
 */

#include "core/epic_harness.h"

/**
 * @brief  Target implementation of harness start-up: no-op, the CPU
 *         starts itself.
 * @param cycles host-only run bound, unused on a real target.
 */
void epic_harness_init(uint32_t cycles)
{
    (void)cycles;   /* The target starts itself; cycles are a host concept. */
}

/**
 * @brief  Target implementation of the time pump: no-op, real time
 *         advances on its own.
 */
void epic_harness_tick(void)
{
    /* Real time advances on its own, nothing to pump. */
}

/**
 * @brief  Target implementation of the loop-continuation test.
 * @param iteration current loop index, unused on a real target.
 * @return always 1; firmware runs forever.
 */
int epic_harness_running(uint32_t iteration)
{
    (void)iteration;
    return 1;       /* Firmware runs forever. */
}

/**
 * @brief  Target implementation of the logger: no-op, there is no
 *         stdout on a real target.
 * @param fmt format string, unused on a real target.
 */
void epic_harness_log(const char *fmt, ...)
{
    (void)fmt;      /* No stdout on the target. */
}
