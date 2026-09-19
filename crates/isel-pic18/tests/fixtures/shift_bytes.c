// epic-cc#470: shift amounts that are a multiple of 8 must lower to byte
// moves, not a bit-serial rotate loop. Covers shl/lshr/ashr, 16- and
// 32-bit widths, plus one non-multiple-of-8 shift (u32_lshr11) to prove
// the residual bit-rotate over the shrunk byte range still works.
volatile unsigned int u16in;
volatile unsigned int u16_lshr8;
volatile unsigned int u16_shl8;

volatile unsigned long u32in;
volatile unsigned long u32_lshr16;
volatile unsigned long u32_shl16;
volatile unsigned long u32_lshr11;

volatile int s16in;
volatile int s16_ashr8;

void main(void) {
    u16_lshr8 = u16in >> 8;
    u16_shl8 = u16in << 8;
    u32_lshr16 = u32in >> 16;
    u32_shl16 = u32in << 16;
    u32_lshr11 = u32in >> 11;
    s16_ashr8 = s16in >> 8;
}
