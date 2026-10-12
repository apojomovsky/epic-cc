#include <stdint.h>
// An asm operand naming a small const needs a RAM copy to address.
// Alloc stages one for consts of 255 bytes or fewer seen only by asm,
// so the substitution succeeds instead of ICEing (epic-cc#1132).
static const uint8_t SMALL[10] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 };
volatile uint8_t out;
int main(void) {
    asm volatile("movf %0, w" :: "m"(SMALL));
    out = 1;
    return 0;
}
