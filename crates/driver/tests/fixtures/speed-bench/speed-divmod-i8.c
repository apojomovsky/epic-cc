/* i8 signed divide and modulo. Pins the 8-bit signed division the
 * 14-bit core lowers to software routines. Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile int8_t a = -100;
volatile int8_t b = 13;
volatile int8_t q;
volatile int8_t m;

void main(void) {
    bench_mark = 1;
    q = (int8_t)(a / b);
    m = (int8_t)(a % b);
    bench_mark = 2;
}
