#include <stdint.h>
/* A constant ROM address stored into a RAM pointer global. */
volatile uint8_t out;
const char *volatile g;
int main(void) {
    g = "EPIC_HARNESS_RESULT: PASS\n";
    out = (uint8_t)g[1];
    return 0;
}
