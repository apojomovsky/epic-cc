#include <stdint.h>

/* RAM path of flash_walk_mixed_store.c (epic-cc#1099, epic-cc#1119). */
volatile uint8_t n = 3;
char buf[4] = "abcd";

int main(void) {
    char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t i;
    for (i = 0; i < n; i++)
        p = buf;
    p[i] = 1;
    return 0;
}
