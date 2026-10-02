/* u8 multiply. Pins the 8x8 hardware multiply on PIC18 (MULWF) and the
 * software multiply loop on PIC14. Plain C99: the private comparison
 * builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint8_t a = 37;
volatile uint8_t b = 29;
volatile uint8_t r;

void main(void) {
    uint8_t x;
    bench_mark = 1;
    x = (uint8_t)(a * b);
    r = x;
    bench_mark = 2;
}
