#include <stdint.h>
/* A store through a runtime-indexed flash walk phi (epic-cc#1101). The
 * literal is ROM, so the driver must refuse rather than write a RAM address. */
volatile uint8_t n;
int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t k;
    n = 3;
    for (k = 0; k < n; k++)
        p = p + 1;
    ((volatile char *)p)[k] = 'x';
    return 0;
}
