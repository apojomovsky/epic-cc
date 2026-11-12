// A struct-array field address carries both a constant field offset
// and a scaled runtime index: &items[idx].g resolves to
// items + 6 + 12*idx. For idx == 23 the byte offset is 282, so the
// low address byte sum carries into the high byte. The sentinel at
// offset 26 is what a dropped carry reads, the sentinel at offset
// 276 is what a dropped constant offset reads, and only the exact
// sum reads the sentinel at offset 282 (epic-cc#1461). The putb
// calls plant the sentinels through the call-argument path, which
// already accumulates multi-term addresses exactly, so setup does
// not ride the pointer-value machinery under test. A field read
// on the small array
// (offset 90 for idx == 23) covers the plain load use in the same
// shape. Expected for in == 23: out == 0x33, out2 == 0x51,
// out4 == 0x51.
struct S { unsigned char a, b, c, d, e, f, g, h, i, j, k, l; };

volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char out2;
volatile unsigned char out4;
volatile struct S items[24];
volatile struct S small[8];
volatile unsigned char *gp;
volatile unsigned char *gp2;

__attribute__((noinline)) static void putb(unsigned char *p, unsigned char v) {
    *p = v;
}

__attribute__((noinline)) static unsigned char read_gp(void) {
    return *gp;
}

__attribute__((noinline)) static unsigned char read_gp2(void) {
    return *gp2;
}

__attribute__((noinline)) static void via_param(struct S *sp,
                                                 unsigned short idx) {
    gp2 = &sp[idx].g;
}

void main(void) {
    unsigned short idx = in;
    putb(&items[idx].g, 0x51);
    putb(&items[2].c, 0x17);
    putb(&items[idx].a, 0x50);
    small[7].g = 0x33;
    small[7].a = 0x32;
    unsigned char j = (unsigned char)(in & 7);
    out = small[j].g;
    gp = &items[idx].g;
    out2 = read_gp();
    via_param(items, idx);
    out4 = read_gp2();
}
