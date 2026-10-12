#include <stdint.h>
/* Two runtime indices stacked on a flash walk phi (epic-cc#1101): `q` is
 * `p + a` and `q[b]` adds a second runtime term. */
volatile uint8_t n;
volatile uint8_t a;
volatile uint8_t b;
volatile uint8_t out;
int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t k;
    const char *q;
    n = 3;
    for (k = 0; k < n; k++)
        p = p + 1;
    a = 1;
    b = 2;
    q = p + a;
    out = (uint8_t)q[b];
    return 0;
}
