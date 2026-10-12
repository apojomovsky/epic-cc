#include <stdint.h>
/* Runtime-condition select between a flash walk arm and its offset (epic-cc#1101).
 * `c` is 1, so `q` is `p + 1` and `q[0]` is the second literal byte. */
volatile uint8_t c;
volatile uint8_t out;
int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    const char *q;
    c = 1;
    q = c ? p + 1 : p;
    out = (uint8_t)q[0];
    return 0;
}
