// P3 (PIC14E) regression: a constant-length memcpy whose destination is a
// bank-straddling global must route through FSR0 with the linear base, not
// emit direct file-register stores that walk into the common-RAM hole and
// bank-1 SFRs.
//
// Layout (alloc, region_for): `src[80]` at 0x20-0x6F; `big[90]` straddles
// bank 0 -> bank 1 (0xA0-0xAF then 0x120-0x12F, the common-RAM hole
// 0x70-0x7F skipped), so its linear base is 0x20A0; `out` at 0x130.
//
// The 80-byte copy crosses the bank-0 -> bank-1 boundary (big[0..79] spans
// 0xA0-0xEF), so a direct-store path would walk into the common-RAM hole
// and bank-1 SFRs. The FSR0/linear path compresses the hole and lands every
// byte correctly.
//
// Expected: src[0] = 0x11, src[79] = 0x50, memcpy 80 bytes into big,
// out = big[79] = 0x50.
volatile unsigned char src[80];
volatile unsigned char big[90];
volatile unsigned char out;

void main(void) {
    src[0] = 0x11;
    src[79] = 0x50;
    __builtin_memcpy((void *)big, (void *)src, 80);
    out = big[79];
}
