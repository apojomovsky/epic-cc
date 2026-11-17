// A pointer select between two offsets of one array, materialized into
// a pointer variable. The select folds to the low offset plus a
// condition term. The byte past the 1-byte condition slot holds
// big[0], set to 0x11 below, so a high-byte read past the condition
// slot changes the address the write through gp lands on.
// Expected: p == buf + 2, so out == buf[2] == 0x5A.
volatile unsigned char in = 1;
volatile unsigned char out;
volatile unsigned char buf[16];
volatile unsigned char big[16];
unsigned char * volatile gp;

__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) {
    *p = v;
}

void main(void) {
    put(big + 290, 0x11);
    big[0] = 0x11;
    unsigned char sel = (unsigned char)(in & 1);
    unsigned char *p = (sel == 0) ? buf + 3 : buf + 2;
    gp = p;
    put(gp, 0x5A);
    out = buf[2];
}
