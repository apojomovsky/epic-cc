/* A runtime-indexed read through a flash-or-RAM walk phi (epic-cc#1099).
 * `p[i]` has no single address space: flash literal on one path, RAM on the other. */
#include <stdint.h>

volatile uint8_t n;
volatile uint8_t out;
char buf[4];

int main(void) {
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    out = (uint8_t)p[i];
    return 0;
}
