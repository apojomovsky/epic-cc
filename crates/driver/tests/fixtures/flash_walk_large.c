#include <stdint.h>

/* An index walk over a flash string too long for a RAM copy (303 bytes,
 * over the 255-byte copy limit). The index stays a 16-bit register over
 * one base, so each load reads the chunked RETLW table: no walk slot holds
 * a flash address, and the PIC14 family must keep compiling it
 * (epic-cc#1197, epic-cc#1203). */
#define T10 "0123456789"
#define T100 T10 T10 T10 T10 T10 T10 T10 T10 T10 T10

static const char big[] = T100 T100 T100 "end";

volatile uint8_t out;
volatile uint8_t sent;

int main(void)
{
    const char *s = big;
    int len = 0;
    while (s[len] != '\0') {
        out = (uint8_t)s[len];
        len++;
    }
    sent = (uint8_t)len;
    return 0;
}
