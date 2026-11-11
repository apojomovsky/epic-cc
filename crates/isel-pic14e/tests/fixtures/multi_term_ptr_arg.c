// A pointer built by two chained runtime offsets and passed as a plain
// call argument folds to a base plus two dynamic terms. The stepwise
// `p +=` statements keep clang from merging the offsets into one index,
// so the call site sees both terms (epic-cc#1447). One call chain starts
// from a global array, the other forwards through a pointer parameter,
// and the wide array makes both terms scaled. Expected for in == 43:
// i == 3, j == 5, offset 8, out == 0x2A + 0x37 + 0x34 == 0x95.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char buf[32];
volatile unsigned char buf2[32];
volatile unsigned short wide[32];

__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) {
    *p = v;
}

__attribute__((noinline)) static void put16(unsigned short *p, unsigned short v) {
    *p = v;
}

// External linkage keeps `p` a real pointer parameter: an internal
// function would let clang specialize it to the one global it is fed.
__attribute__((noinline)) void put_fwd(unsigned char *p, unsigned char i,
                                              unsigned char j, unsigned char v) {
    p += i;
    p += j;
    put(p, v);
}

void main(void) {
    unsigned char i = (unsigned char)(in & 7);
    unsigned char j = (unsigned char)((in >> 3) & 7);
    unsigned char *p = buf + i;
    p += j;
    put(p, 0x2A);
    put_fwd(buf2, i, j, 0x37);
    unsigned short *w = wide + i;
    w += j;
    put16(w, 0x1234);
    out = (unsigned char)(buf[8] + buf2[8] + (wide[8] & 0xFF));
}
