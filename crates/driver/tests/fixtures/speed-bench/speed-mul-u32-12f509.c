/* u32 multiply with a constant operand, 12F509 row (epic-cc#1179). Pins
 * the same __mul_u32 shift-add loop as speed-mul-u32.c: the volatile load
 * keeps the product data-dependent, so neither profile folds it. The full
 * two-volatile kernel cannot allocate on the 509 (two live 4-byte operands
 * plus the 12-byte routine frame exceed the 16-byte bank), so this reduced
 * kernel carries the 509 row with the identical product. Plain C99: the
 * private comparison builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint32_t a = 0x12345678UL;
volatile uint32_t r;

void main(void) {
    bench_mark = 1;
    r = a * 0x9ABCDEF1UL;
    bench_mark = 2;
}
