#include <stdint.h>
static const unsigned char table[32] = { 0x40, 0, 0, 0 };
const unsigned char * volatile srcp = table;
volatile unsigned char buf[32];
volatile uint16_t v;
volatile unsigned char sink;
void main(void)
{
    __builtin_memcpy(buf, srcp, 32);
    __builtin_memcpy(&v, buf, 2);
    sink = *((volatile unsigned char *)(unsigned long)v);
    for (;;) {}
}
