// Including <string.h> links the driver's string unit, but nothing
// here calls memset by name: only the struct zero-initialization emits
// llvm.memset (epic-cc#1448). The library memset must survive the
// whole-program opt pass for the rewritten call to reach it.
// out == 0xA5 + 5 + 0 + 0 == 0xAA, as in memset_intrinsic.c.
#include <string.h>

volatile unsigned char in = 5;
volatile unsigned char out;

struct Big { unsigned char b[24]; };

static struct Big big;
static unsigned char buf[24];

void main(void) {
    big.b[3] = 0x77;
    big = (struct Big){0};
    __builtin_memset(big.b, in, 4);
    __builtin_memset(buf, 0xA5, in);
    out = (unsigned char)(buf[4] + big.b[3] + big.b[10] + buf[5]);
}
