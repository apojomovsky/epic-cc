#include <stdint.h>

/* A `const char *` walk over a flash string with no call argument (#1091):
 * the walk pointer is a phi over the literal and its own gep, so every
 * load through it must read program memory. */
volatile uint8_t out;
volatile uint8_t sent;

int main(void)
{
    const char *s = "EPIC_HARNESS_RESULT: PASS\n";
    int len = 0;
    while (s[len] != '\0') {
        out = (uint8_t)s[len];
        len++;
    }
    sent = (uint8_t)len;
    return 0;
}
