#include <stdint.h>

/* RAM path of flash_walk_mixed_index.c (epic-cc#1099, epic-cc#1119). */
volatile uint8_t n = 2;
volatile uint8_t out;
char buf[4] = "abcd";

int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    out = (uint8_t)p[i];
    return 0;
}
