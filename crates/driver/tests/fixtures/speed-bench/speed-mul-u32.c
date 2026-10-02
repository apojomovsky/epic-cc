/* u32 multiply. Pins the 32-bit widening multiply (successive MULWF
 * on PIC18, shift-add loop on PIC14). Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint32_t a = 0x12345678UL;
volatile uint32_t b = 0x9ABCDEF1UL;
volatile uint32_t r;

void main(void) {
    uint32_t x;
    bench_mark = 1;
    x = a * b;
    r = x;
    bench_mark = 2;
}
