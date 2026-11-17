// Zero-initializing a struct and calling __builtin_memset make clang
// emit llvm.memset intrinsics (epic-cc#1448); no <string.h> is included,
// so the memset implementation has to arrive with the intrinsic. With
// in == 5: big is zeroed, its first four bytes refilled with 5, and
// buf's first five bytes filled with 0xA5, so
// out == 0xA5 + 5 + 0 + 0 == 0xAA.
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
