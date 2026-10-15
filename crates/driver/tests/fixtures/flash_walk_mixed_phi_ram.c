#include <stdint.h>

/* RAM path of flash_walk_mixed_phi.c (epic-cc#1099, epic-cc#1119): `n`
 * is initialized so the loop reassigns `p` to `buf`, pinning the buffer
 * path through the same mixed phi that the zero-`n` run takes literaly. */
volatile uint8_t n = 3;
volatile uint8_t out;
char buf[4] = "abcd";

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    out = (uint8_t)p[0];
    return 0;
}
