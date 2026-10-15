#include <stdint.h>

/* RAM path of flash_walk_mixed_memcpy.c (epic-cc#1100, epic-cc#1119). */
volatile uint8_t n = 3;
volatile uint8_t out;
uint8_t buf[16] = "0123456789abcdef";
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
