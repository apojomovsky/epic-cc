/* A runtime-indexed store through a flash-or-RAM walk phi (epic-cc#1099).
 * `p[i] = 1` would write RAM at the flash literal's address on one path. */
#include <stdint.h>

volatile uint8_t n;
char buf[4];

int main(void) {
    char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    p[i] = 1;
    return 0;
}
