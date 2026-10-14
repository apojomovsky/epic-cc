#include <stdint.h>

/* Pure-flash callee walk (epic-cc#1092, epic-cc#1119): two flash literals
 * reach one non-inlined pointer parameter, so the slot is flash-kind and
 * the callee walks it through TBLPTR. Neither literal keeps a RAM copy.
 * This side takes the long literal. */
volatile uint8_t out;
volatile uint8_t n = 1;
volatile uint8_t sel = 1;

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
