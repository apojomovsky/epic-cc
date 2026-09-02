// epic-cc#131 acceptance: variadic printf with %ld/%u formats on both
// cores. printf writes through the retargetable putchar sink into g_buf;
// main stores the char count at g_n so the sim test can find the output.
#include <stdio.h>

volatile char g_buf[80];
volatile int g_n = 0;

int putchar(int c) {
    g_buf[g_n] = (char)c;
    g_n++;
    return c;
}

void main(void) {
    long pos = 123456L;
    unsigned int err = 7u;
    unsigned int glitch = 9u;
    int n = printf("pos=%ld err=%u glitch=%u\r\n", pos, err, glitch);
    g_buf[n] = 0;
    for (;;) {
    }
}
