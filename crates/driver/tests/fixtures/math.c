#include <math.h>

// Math library acceptance: each routine on an exact value, one volatile
// global per result so no call is DCE'd. Every argument derives from the
// volatile input so clang keeps real calls. Expected (hand-computed):
//   sqrtf(2.25) = 1.5, sqrtf(0.25) = 0.5, fabsf(-2.25) = 2.25,
//   fmaxf(1.5, 2.25) = 2.25, floorf(2.99) = 2.0, floorf(-2.25) = -3.0.
volatile float in;
volatile float out_sqrt;
volatile float out_sqrt_small;
volatile float out_fabs;
volatile float out_fmax;
volatile float out_floor;
volatile float out_floor_neg;
void main(void) {
    in = 2.25f;
    out_sqrt = sqrtf(in);
    out_sqrt_small = sqrtf(in - 2.0f);
    out_fabs = fabsf(0.0f - in);
    out_fmax = fmaxf(out_sqrt, out_fabs);
    out_floor = floorf(in + 0.74f);
    out_floor_neg = floorf(0.0f - in);
    for (;;) {}
}
