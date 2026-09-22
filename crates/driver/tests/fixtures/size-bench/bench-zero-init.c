#include <stdint.h>
volatile uint16_t out16;
volatile uint32_t out32;
uint16_t z16;
uint32_t z32;
void main(void) {
    z16 = 0;
    z32 = 0;
    out16 = z16;
    out32 = z32;
}
