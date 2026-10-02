/* u16 divide and modulo. The decimal engine (speed-u16-dec) and the
 * menu heartbeat both repeat this shape, so it gets its own row.
 * Plain C99: the private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint16_t a = 50000;
volatile uint16_t b = 137;
volatile uint16_t q;
volatile uint16_t m;

void main(void) {
    bench_mark = 1;
    q = (uint16_t)(a / b);
    m = (uint16_t)(a % b);
    bench_mark = 2;
}
