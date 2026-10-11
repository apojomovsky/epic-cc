#include <stdint.h>

/* A constant-length `memcpy` from a `const char *` walk over a flash string
 * (epic-cc#1100). The copy must read program memory: `buf[0]` holds the
 * walk's current character on every step, so `sum` pins each step. */
volatile uint8_t sum;
volatile uint8_t sent;
uint8_t buf[16];

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t count = 0;
    uint8_t total = 0;
    while (p[0] != '\0') {
        __builtin_memcpy(buf, p, 16);
        total += buf[0];
        p++;
        count++;
    }
    sum = total;
    sent = count;
    return 0;
}
