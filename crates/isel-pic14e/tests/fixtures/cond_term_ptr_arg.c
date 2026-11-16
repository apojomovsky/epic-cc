// A pointer select between two offsets of one array, passed as a plain
// call argument. The select folds to the low offset plus a condition
// term. The byte past the 1-byte condition slot is the callee's
// second parameter slot, which the first call leaves holding 0x11,
// so a high-byte read past the condition slot changes the address
// the second call writes through.
// Expected for in == 1: p == buf + 2, so out == buf[2] == 0x5A.
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[16];
volatile unsigned char big[16];

__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) {
    *p = v;
}

void main(void) {
    put(big + 290, 0x11);
    unsigned char sel = (unsigned char)(in & 1);
    unsigned char *p = (sel == 0) ? buf + 3 : buf + 2;
    put(p, 0x5A);
    out = buf[2];
}
