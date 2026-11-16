// A GEP term that is also an earlier call argument: the earlier
// argument is stacked into its parameter slot first, and the term
// is read from a slot that can overlay that parameter slot. The
// stacked value and the term are the same register, so the term
// must still read its own value. Expected for in == 1 and
// in2 == 5: out == 0x5A and out2 == 5.
volatile unsigned char in;
volatile unsigned short in2;
volatile unsigned char out;
volatile unsigned char out2;
volatile unsigned char buf[16];
volatile unsigned char big[16];
__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) { *p = v; }
__attribute__((noinline)) static void put2(unsigned short t, unsigned char *p, unsigned char v) {
    *p = v;
    out2 = (unsigned char)t;
}
void main(void) {
    put(big + 290, 0x11);
    unsigned short t = in2;
    unsigned char sel = (unsigned char)(in & 1);
    unsigned char *p = (sel == 0) ? buf + 3 : buf + 2;
    unsigned char *q = p + t;
    put2(t, q, 0x5A);
    out = buf[7];
}
