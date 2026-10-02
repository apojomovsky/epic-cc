/* strlen plus strcmp. Both strings are built from a volatile seed
 * inside the timed region so neither call folds; the strings compare
 * equal, forcing strcmp to walk the full length. Plain C99: the
 * private comparison builds this file unchanged. */
#include <stdint.h>
#include <string.h>

volatile unsigned char bench_mark;
volatile uint8_t seed = 0x30;
char s1[16];
char s2[16];
volatile uint8_t result;

void main(void) {
    uint8_t i;
    uint8_t n;
    int c;
    bench_mark = 1;
    for (i = 0; i < 15; i++) {
        s1[i] = (char)(seed + (i & 7u));
        s2[i] = (char)(seed + (i & 7u));
    }
    s1[15] = 0;
    s2[15] = 0;
    n = (uint8_t)strlen(s1);
    n = (uint8_t)(n + strlen(s2));
    c = strcmp(s1, s2);
    result = (uint8_t)(n + (uint8_t)c);
    bench_mark = 2;
}
