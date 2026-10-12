#include <stdint.h>
// An asm operand naming a const over 255 bytes has no RAM copy to address.
// Alloc stages only consts of 255 bytes or fewer, so the operand lookup must
// fail with a user error naming the const, never an ICE (epic-cc#1115).
static const uint8_t BIG[300] = { [0] = 0x41 };
volatile uint8_t out;
int main(void) {
    asm volatile("movf %0, w" :: "m"(BIG));
    out = 1;
    return 0;
}
