/*
 * Shared portable-C RNG (no asm): epic_math_rand_next is a 16-bit
 * Fibonacci LFSR (taps {0,2,3,5}, period 65535), not cryptographic. A
 * zero *state maps to a fixed nonzero seed, escaping the LFSR's
 * unrecoverable all-zero fixed point. epic_math_rand_gauss sums four
 * samples (Central Limit Theorem) for an approximately bell-shaped
 * distribution, mirroring AN544 Figure 3.
 */

#include "epic_math.h"

#define EPIC_MATH_RAND_SEED 0xACE1u   /* documented nonzero seed for state==0 */

/**
 * @brief  16-bit maximal-length LFSR pseudo-random step.
 * @param  state  in/out LFSR state; must point at a persistent uint16_t.
 *                A zero state is treated as the documented nonzero seed.
 * @return the next 16-bit pseudo-random value (also written back to
 *         *state). Never 0 over the full 65535-period cycle.
 */
uint16_t epic_math_rand_next(uint16_t *state)
{
    uint16_t s = *state;
    if (s == 0u)
    {
        s = EPIC_MATH_RAND_SEED;      /* escape the all-zero fixed point      */
    }
    /* maximal-length 16-bit LFSR, taps {0,2,3,5} -> x^16+x^14+x^13+x^11+1 */
    uint16_t bit = (uint16_t)(((s >> 0) ^ (s >> 2) ^ (s >> 3) ^ (s >> 5)) & 1u);
    s = (uint16_t)((s >> 1) | (bit << 15));
    *state = s;
    return s;
}

/**
 * @brief  Approximate Gaussian (mean 0) pseudo-random sample via the
 *         Central Limit Theorem: the sum of four LFSR samples, normalized.
 * @param  state  in/out LFSR state shared with epic_math_rand_next.
 * @return a signed sample with an approximately bell-shaped distribution.
 */
int16_t epic_math_rand_gauss(uint16_t *state)
{
    int32_t sum = 0;
    for (int i = 0; i < 4; i++)
    {
        sum += (int32_t)epic_math_rand_next(state);
    }
    sum -= 131072;   /* 4 * 32768 (mean of the 1..65535 uniform LFSR output) */
    return (int16_t)(sum >> 2);   /* scale the +/-131070 range into int16     */
}
