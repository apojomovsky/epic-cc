#include <stdint.h>

/* The load goes through a constant-offset gep over the walk phi (`q = p + 1;
 * q[1]`), not the phi itself (#1091). The gep has no slot of its own, so the
 * byte must be read through the phi's slot at the folded offset. */
volatile uint8_t sum;
volatile uint8_t n;

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t acc = 0;
    uint8_t count = 0;
    while (p[1] != '\0') {
        const char *q = p + 1;
        acc = (uint8_t)(acc + (uint8_t)q[1]);
        p = q;
        count++;
    }
    sum = acc;
    n = count;
    return 0;
}
