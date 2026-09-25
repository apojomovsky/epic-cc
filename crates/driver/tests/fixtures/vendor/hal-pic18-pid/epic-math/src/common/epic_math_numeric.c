/*
 * Shared portable-C numeric routines (no asm): epic_math_diff3 (3-point
 * central derivative) and epic_math_integrate_simpson38 (Simpson's-3/8
 * integration), fixed-point, one implementation linked by every backend.
 * The caller passes precomputed Q-format scale factors; see
 * include/epic_math.h for the exact Q-format of each argument.
 */

#include "epic_math.h"

/**
 * @brief  3-point central first derivative: (x_now - x_prev2) / (2h) in
 *         fixed point; see include/epic_math.h for the Q-format contract.
 * @param  x_prev2      sample at t - h
 * @param  x_prev1      sample at t (midpoint; unused by the formula)
 * @param  x_now        sample at t + h
 * @param  inv_2h_q8    Q8.8 fixed-point 1/(2h)
 * @return Q8.8 estimate of the derivative at the midpoint.
 */
int16_t epic_math_diff3(int16_t x_prev2, int16_t x_prev1, int16_t x_now,
                       int16_t inv_2h_q8)
{
    /* x_prev1 (the midpoint) is unused by the central-difference formula,
     * kept in the signature for the 3-sample window context. Result
     * truncates to int16 if it exceeds the Q8.8 range (documented). */
    (void)x_prev1;
    int32_t diff = (int32_t)x_now - (int32_t)x_prev2;
    int32_t prod = diff * (int32_t)inv_2h_q8;
    return (int16_t)prod;
}

/**
 * @brief  Simpson's-3/8-rule integration over four samples in fixed
 *         point; see include/epic_math.h for the Q-format contract.
 * @param  f0  sample at t
 * @param  f1  sample at t + h
 * @param  f2  sample at t + 2h
 * @param  f3  sample at t + 3h
 * @param  three_h_over_8_q16  Q16.16 fixed-point 3h/8
 * @return Q16.16 estimate of the integral.
 */
int32_t epic_math_integrate_simpson38(int16_t f0, int16_t f1, int16_t f2,
                                     int16_t f3, int32_t three_h_over_8_q16)
{
    /* Computed in 32-bit (no 64-bit type on PIC16 XC8): the caller must
     * scale inputs so the Q16.16 result fits int32, or it truncates. */
    int32_t sum = (int32_t)f0 + 3 * (int32_t)f1 + 3 * (int32_t)f2 + (int32_t)f3;
    int32_t prod = three_h_over_8_q16 * sum;
    return prod;
}
