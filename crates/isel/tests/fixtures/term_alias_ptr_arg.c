// A select between two offsets of one array, then a further runtime
// offset, folds to a multi-term pointer passed as a call argument.
// One term's slot overlays the callee's first parameter slot, so
// seeding the base into that slot must not destroy the term before
// it is added. Expected for in == 1 and in2 == 0: out == 0x5A.
volatile unsigned char in;
volatile unsigned short in2;
volatile unsigned char out;
volatile unsigned char buf[16];
volatile unsigned char big[16];
__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) { *p = v; }
void main(void) {
    put(big + 290, 0x11);
    unsigned char sel = (unsigned char)(in & 1);
    unsigned char *p = (sel == 0) ? buf + 3 : buf + 2;
    unsigned char *q = p + in2;
    put(q, 0x5A);
    out = buf[2];
}
