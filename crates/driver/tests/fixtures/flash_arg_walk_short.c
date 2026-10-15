#include <stdint.h>

/* Short-literal side of flash_arg_walk.c (epic-cc#1092, epic-cc#1119). */
volatile uint8_t out;
volatile uint8_t n = 1;
volatile uint8_t sel = 0;

__attribute__((noinline)) static uint8_t at(const char *s, uint8_t i) {
    return (uint8_t)s[i];
}

int main(void) {
    if (sel) {
        out = at("EPIC_HARNESS_RESULT: PASS\n", n);
    } else {
        out = at("ok\n", n);
    }
    return 0;
}
