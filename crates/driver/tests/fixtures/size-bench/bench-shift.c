#include <stdint.h>
volatile uint16_t out16;
volatile uint32_t out32;
uint16_t g16;
uint32_t g32;
void main(void) {
    g16 = (uint16_t)(g16 << 4);
    g16 = (uint16_t)(g16 >> 2);
    g32 = g32 << 6;
    g32 = g32 >> 4;
    out16 = g16;
    out32 = g32;
}
