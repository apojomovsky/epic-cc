/* i16 signed divide and modulo. Pins the 16-bit signed division the
 * 14-bit core lowers to software routines. Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile int16_t a = -30000;
volatile int16_t b = 137;
volatile int16_t q;
volatile int16_t m;

void main(void) {
    bench_mark = 1;
    q = (int16_t)(a / b);
    m = (int16_t)(a % b);
    bench_mark = 2;
}
