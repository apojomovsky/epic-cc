// A program defining its own memset, reached only through the
// llvm.memset intrinsic from struct zero-initialization
// (epic-cc#1448). The definition fills with c + 1, so a correctly
// preserved body leaves big.b[2] == 1 after the zero-initialization;
// a body folded away by the whole-program opt pass either hangs the
// call or leaves 0. No <string.h>: the rescue unit must not link, and
// the user's definition must survive on its own.
#include <stddef.h>

void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char *)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)(c + 1);
    return s;
}

struct Big { unsigned char b[48]; };

static struct Big big;
volatile unsigned char out;

void main(void) {
    big = (struct Big){0};
    out = big.b[2];
}
