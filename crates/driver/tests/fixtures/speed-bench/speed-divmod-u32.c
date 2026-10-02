/* u32 divide and modulo. The slowest single operation in the ladder
 * on PIC14 (32-bit software division). Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint32_t a = 3000000000UL;
volatile uint32_t b = 1234567UL;
volatile uint32_t q;
volatile uint32_t m;

void main(void) {
    bench_mark = 1;
    q = a / b;
    m = a % b;
    bench_mark = 2;
}
