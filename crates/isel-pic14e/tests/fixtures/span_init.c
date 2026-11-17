// epic-cc#1477: an initialized bank-straddling array in bank 2, so its
// startup bytes cross the 0x170 common-RAM alias window. The two fillers
// occupy banks 0 and 1 exactly, so `zbig` starts at 0x120 and straddles
// bank 2 -> bank 3 (0x120-0x16F then 0x1A0-0x1A9, the gap skipped).
//
// Expected: zbig[0]=0x11, zbig[79]=0x22, zbig[80]=0x33, zbig[89]=0x44,
// out = 0x11+0x22+0x33+0x44 = 0xAA (fillers read as zero).
volatile unsigned char fill0[80] = { [79] = 0x01 };
volatile unsigned char fill1[80] = { [79] = 0x02 };
volatile unsigned char zbig[90] = {
    [0] = 0x11, [79] = 0x22, [80] = 0x33, [89] = 0x44,
};
volatile unsigned char out;
void main(void) {
    out = (unsigned char)(zbig[0] + zbig[79] + zbig[80] + zbig[89]
                          + fill0[0] + fill1[0]);
}
