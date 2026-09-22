#include <stdint.h>
volatile uint16_t out16;
volatile uint32_t out32;
void main(void) {
    out16 = 0x1234;
    out32 = 0x12003400UL;
}
