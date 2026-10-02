/* Variable shifts. Counts come from volatile globals so the shift
 * amount is genuinely unknown; a constant count would fold to a fixed
 * wiring. The masks keep every count in range (no undefined shift).
 * Plain C99: the private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint16_t w16 = 0xABCD;
volatile uint32_t w32 = 0x12345678UL;
volatile uint8_t c16 = 5;
volatile uint8_t c32 = 19;
volatile uint16_t r16;
volatile uint32_t r32;

void main(void) {
    uint16_t x16;
    uint32_t x32;
    bench_mark = 1;
    x16 = (uint16_t)(w16 << (c16 & 15u));
    x16 = (uint16_t)(x16 >> (c16 & 15u));
    x32 = w32 << (c32 & 31u);
    x32 = x32 >> (c32 & 31u);
    r16 = x16;
    r32 = x32;
    bench_mark = 2;
}
