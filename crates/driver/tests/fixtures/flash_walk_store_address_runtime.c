#include <stdint.h>
/* A runtime-indexed ROM address stored into a RAM pointer global. */
volatile uint8_t n;
volatile uint8_t out;
const char *volatile g;
int main(void) {
    n = 3;
    g = "EPIC_HARNESS_RESULT: PASS\n" + n;
    out = (uint8_t)g[0];
    return 0;
}
