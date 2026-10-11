#include <stdint.h>

/* A pointer phi joining a flash string and a RAM buffer (epic-cc#1099). The
 * loop reassigns `p` to `buf` once `n` is non-zero, so `p[0]` cannot be
 * lowered to one address space: it must be a compile error, not a read of
 * RAM at the flash address when `n == 0`. */
volatile uint8_t n;
volatile uint8_t out;
char buf[4];

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    out = (uint8_t)p[0];
    return 0;
}
