#include <stdint.h>

/* A `memcpy` from a pointer phi joining a flash string and a RAM buffer
 * (epic-cc#1100). The source is flash or RAM at run time, so the copy must be
 * refused at compile time, never read through FSR1 at a flash address. */
volatile uint8_t n;
volatile uint8_t out;
uint8_t buf[16];
uint8_t dst[16];

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = (const char *)buf;
    __builtin_memcpy(dst, p, 16);
    out = dst[0];
    return 0;
}
