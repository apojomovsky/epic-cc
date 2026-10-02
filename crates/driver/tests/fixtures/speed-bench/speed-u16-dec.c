/* u16 to decimal. Repeated divide/modulo by 10 into a digit sink:
 * the put_u16 shape the menu heartbeat repeats six times per pass
 * (see bench-u16-dec). The input is volatile so the engine cannot
 * fold. Plain C99: the private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint16_t in = 48293;
volatile unsigned char digits[5];

void main(void) {
    uint16_t v;
    uint8_t i;
    bench_mark = 1;
    v = in;
    for (i = 0; i < 5; i++) {
        digits[i] = (unsigned char)(v % 10u);
        v = (uint16_t)(v / 10u);
    }
    bench_mark = 2;
}
