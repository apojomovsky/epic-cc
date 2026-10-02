/* u8 divide and modulo. Pins the 8-bit software division both cores
 * lower to (no hardware divider on either target). Plain C99: the
 * private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint8_t a = 200;
volatile uint8_t b = 13;
volatile uint8_t q;
volatile uint8_t m;

void main(void) {
    bench_mark = 1;
    q = (uint8_t)(a / b);
    m = (uint8_t)(a % b);
    bench_mark = 2;
}
