// A program defining its own strcpy next to an intrinsic-only
// memset use (epic-cc#1448): linking the memset rescue unit must not
// collide with the user's string function. The user's strcpy copies
// one byte and appends 'x', so out == 0 + 'x' == 120 only when both
// the zero-initialization and the user's strcpy really run.
#include <stddef.h>

char *strcpy(char *d, const char *s) {
    d[0] = s[0];
    d[1] = 'x';
    d[2] = 0;
    return d;
}

struct Big { unsigned char b[48]; };

static struct Big big;
static volatile char dst[4];
static const char src[2] = {'q', 0};
volatile unsigned char out;

void main(void) {
    big.b[1] = 9;
    big = (struct Big){0};
    strcpy((char *)dst, src);
    out = (unsigned char)(big.b[1] + (unsigned char)dst[1]);
}
