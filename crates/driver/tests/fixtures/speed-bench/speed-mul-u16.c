/* u16 multiply. Pins the __mul_u16 widening sequence on PIC18 (three
 * MULWF plus two correction adds) and the 16-bit software loop on
 * PIC14. Plain C99: the private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint16_t a = 0x1234;
volatile uint16_t b = 37;
volatile uint16_t r;

void main(void) {
    uint16_t x;
    bench_mark = 1;
    x = (uint16_t)(a * b);
    r = x;
    bench_mark = 2;
}
