// epic-cc#1468: an initialized bank-straddling array on PIC16F877A, so
// its startup bytes cross the 0x170 common-RAM alias window. The array
// is pinned at 0x160, so it straddles bank 2 -> bank 3 (0x160-0x16F
// then 0x190-0x1D9, the gap skipped).
//
// Expected: zbig[0]=0x11, zbig[15]=0x22, zbig[16]=0x33, zbig[89]=0x44,
// out = 0x11+0x22+0x33+0x44 = 0xAA.
volatile unsigned char zbig[90] __attribute__((section(".epicat.0x160"))) = {
    [0] = 0x11, [15] = 0x22, [16] = 0x33, [89] = 0x44,
};
volatile unsigned char out;
void main(void) {
    out = (unsigned char)(zbig[0] + zbig[15] + zbig[16] + zbig[89]);
}
