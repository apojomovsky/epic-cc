#include <stdint.h>

/* A pointer walk over a flash string too long for a RAM copy (303 bytes,
 * over the 255-byte copy limit): the walk slot would hold a flash address
 * that INDF reads as RAM, so the PIC14 family refuses it (epic-cc#1197,
 * epic-cc#1203). */
#define T10 "0123456789"
#define T100 T10 T10 T10 T10 T10 T10 T10 T10 T10 T10

static const char big[] = T100 T100 T100 "end";

volatile uint8_t out;

int main(void)
{
    const char *s = big;
    while (*s) {
        out = (uint8_t)*s;
        s++;
    }
    return 0;
}
