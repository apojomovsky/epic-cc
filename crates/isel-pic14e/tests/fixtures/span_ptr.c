// P3 (PIC14E) regression: a runtime pointer VALUE to a bank-straddling
// global must carry the linear alias, so a deref at an offset past the bank
// boundary walks the linear region (which compresses the common-RAM hole),
// not the hole itself. The pointer is stored in a volatile global and read
// back, so it is genuinely runtime (clang cannot fold it to a static GEP).
//
// Layout (alloc, region_for): `gp` (ptr) at 0x20-0x21; `big[90]` straddles
// bank 0 -> bank 1 (0x22-0x6F then 0xA0-0xAF, the common-RAM hole 0x70-0x7F
// skipped), so its linear base is 0x2002; `out` at 0xB4.
//
// `sum` reads big[0] and big[89] through the pointer param `p`. big[89] is
// at linear 0x2002 + 89 = 0x205B -> physical 0xAB (bank 1). If the pointer
// value were the physical base 0x22, big[89] would compute FSR0 = 0x22+0x59
// = 0x007B, the common-RAM hole, and read garbage.
//
// Expected: big[0] = 0x11, big[89] = 0x22, out = 0x11 + 0x22 = 0x33.
volatile unsigned char *gp;
volatile unsigned char big[90];
volatile unsigned char out;

unsigned char sum(volatile unsigned char *p) {
    return (unsigned char)(p[0] + p[89]);
}

void main(void) {
    big[0] = 0x11;
    big[89] = 0x22;
    gp = big;
    out = sum(gp);
}
