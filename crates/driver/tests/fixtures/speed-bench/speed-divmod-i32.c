/* i32 signed divide and modulo. Pins the 32-bit signed division the
 * 14-bit core lowers to software routines. Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile int32_t a = -123456789;
volatile int32_t b = 256;
volatile int32_t q;
volatile int32_t m;

void main(void) {
    bench_mark = 1;
    q = (int32_t)(a / b);
    m = (int32_t)(a % b);
    bench_mark = 2;
}
