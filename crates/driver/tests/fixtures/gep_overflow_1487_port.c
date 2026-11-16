#include "gep_overflow_1487.h"

/* epic-cc#1487: a port read copies into its buf argument at a runtime
   index, the construct the ros2serial firmware stopped on. The bytes
   are 1..max, a valid backward-decode input for the decode unit.
   g_seed keeps max runtime-unknown (7 at power-on), so the walks
   survive whole-program optimization as pointer walks. */
static struct framer g_f;
volatile unsigned char g_seed;
volatile unsigned char g_out;
volatile unsigned char g_done;

__attribute__((noinline)) unsigned short port_read(unsigned char *buf, unsigned short max)
{
    unsigned short n = 0u;
    while (n < max) {
        buf[n] = (unsigned char)(n + 1u);
        n++;
    }
    return n;
}

void main(void)
{
    unsigned short max = (unsigned short)(7u + (g_seed & 1u));
    g_out = poll(&g_f, max);
    g_done = (unsigned char)(g_out ^ 0xA5u);
}
