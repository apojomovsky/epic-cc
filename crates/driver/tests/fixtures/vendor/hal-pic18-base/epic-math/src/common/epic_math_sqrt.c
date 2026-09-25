/*
 * Shared portable-C integer square root (no asm): Newton-Raphson built
 * on epic_math_divmod_u16 rather than fresh asm, one implementation
 * linked by every backend. Converges to floor(sqrt(value)) for the full
 * 0..65535 range.
 */

#include "epic_math.h"
#include <stddef.h>     /* NULL (passed to divmod_u16 for the ok-out flag) */

/**
 * @brief  floor(sqrt(value)) for 0..65535 via 16-bit Newton-Raphson
 *         built on epic_math_divmod_u16.
 * @param  value  value to take the square root of, 0..65535
 * @return floor(sqrt(value)), 0..255.
 */
uint16_t epic_math_sqrt_u16(uint16_t value)
{
    if (value < 2u)
    {
        return value;   /* 0 -> 0, 1 -> 1 */
    }
    /* Newton-Raphson: x_{k+1} = (x_k + n/x_k) / 2, converging down to
     * floor(sqrt(n)). Start from n (an over-estimate) and iterate while the
     * estimate keeps decreasing. */
    uint16_t x = value;
    uint16_t y = (uint16_t)((x + 1u) / 2u);
    while (y < x)
    {
        x = y;
        epic_math_udiv16_t d = epic_math_divmod_u16(value, x, NULL);
        y = (uint16_t)((x + d.quotient) / 2u);
    }
    return x;
}
