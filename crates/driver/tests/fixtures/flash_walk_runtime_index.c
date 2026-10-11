#include <stdint.h>
/* Runtime-indexed read over a flash walk phi (epic-cc#1101): the loop
 * advances `p` three times, then `p[k]` reads at a runtime index. */
volatile uint8_t n;
volatile uint8_t out;
int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t k;
    n = 3;
    for (k = 0; k < n; k++)
        p = p + 1;
    out = (uint8_t)p[k];
    return 0;
}
