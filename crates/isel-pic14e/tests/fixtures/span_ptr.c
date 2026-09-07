// P3 (PIC14E) regression: a runtime pointer VALUE to a bank-straddling
// global must carry the linear alias, so a deref at an offset past the bank
// boundary walks the linear region (which compresses the common-RAM hole),
// not the hole itself. The pointer is stored in a volatile global and read
// back, so it is genuinely runtime (clang cannot fold it to a static GEP).
//
// Layout (alloc, region_for): `big[90]` at 0x20 straddles bank 0 -> the
// common-RAM hole (0x20-0x6F then 0x70-0x7F skipped, 0xA0-0xAF), so its
// linear base is 0x2000; `gp` (ptr) at 0x2A-0x2B (bank 0); `out` at 0x2C
// (bank 0).
//
// `sum` reads big[0] and big[89] through the pointer param `p`. big[89] is
// at linear 0x2000 + 89 = 0x2059 -> physical 0xA9 (bank 1). If the pointer
// value were the physical base 0x20, big[89] would compute FSR0 = 0x20+0x59
// = 0x0079, the common-RAM hole, and read garbage. `sum` is noinline so the
// pointer is genuinely runtime (clang -O1 would otherwise inline it and
// dereference @big directly, never exercising the pointer-value path).
//
// Expected: big[0] = 0x11, big[89] = 0x22, out = 0x11 + 0x22 = 0x33.
volatile unsigned char *gp;
volatile unsigned char big[90];
volatile unsigned char out;

__attribute__((noinline)) unsigned char sum(volatile unsigned char *p) {
    return (unsigned char)(p[0] + p[89]);
}

void main(void) {
    big[0] = 0x11;
    big[89] = 0x22;
    gp = big;
    out = sum(gp);
}
