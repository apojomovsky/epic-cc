// A pointer built by two chained runtime offsets and passed as a plain
// call argument folds to a base plus two dynamic terms. The stepwise
// `p +=` statements keep clang from merging the offsets into one index,
// so the call site sees both terms (epic-cc#1447). One call chain starts
// from a global array, the other forwards through a pointer parameter,
// and the wide array makes both terms scaled. The big array repeats
// the shape with two 200 offsets, whose sum carries out of the low
// address byte at any base. Expected for in == 43 and in2 == 0xC8C8:
// i == 3, j == 5, offset 8, the sum 0x2A + 0x37 + 0x34 + 0xA5 lands in out as 0x3A.
volatile unsigned short in;
volatile unsigned short in2;
volatile unsigned char out;
volatile unsigned char buf[16];
volatile unsigned char buf2[16];
volatile unsigned short wide[16];
volatile unsigned char big[402];

__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) {
    *p = v;
}

__attribute__((noinline)) static void put16(unsigned short *p, unsigned short v) {
    *p = v;
}

__attribute__((noinline)) static unsigned char get(unsigned char *p) {
    return *p;
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
    unsigned char hi = (unsigned char)(in2 & 0xFF);
    unsigned char hj = (unsigned char)((in2 >> 8) & 0xFF);
    unsigned char *q = big + hi;
    q += hj;
    put(q, 0x5A);
    put_fwd(big, hi, hj, 0xA5);
    out = (unsigned char)(buf[8] + buf2[8] + (wide[8] & 0xFF) + get(q));
}
