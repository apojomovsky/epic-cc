// epic-cc#470: shift amounts that are a multiple of 8 must lower to byte
// moves, not a bit-serial rotate loop. Covers shl/lshr/ashr, 16- and
// 32-bit widths. The non-multiple-of-8 shifts (u32_lshr11, u32_shl12,
// s16_ashr12) prove the residual bit-rotate over the shrunk byte range
// still works, for every op and on both sides of the byte-move index.
volatile unsigned int u16in;
volatile unsigned int u16_lshr8;
volatile unsigned int u16_shl8;

volatile unsigned long u32in;
volatile unsigned long u32_lshr16;
volatile unsigned long u32_shl16;
volatile unsigned long u32_lshr11;
volatile unsigned long u32_shl12;

volatile int s16in;
volatile int s16_ashr8;
volatile int s16_ashr12;

void main(void) {
    u16_lshr8 = u16in >> 8;
    u16_shl8 = u16in << 8;
    u32_lshr16 = u32in >> 16;
    u32_shl16 = u32in << 16;
    u32_lshr11 = u32in >> 11;
    u32_shl12 = u32in << 12;
    s16_ashr8 = s16in >> 8;
    s16_ashr12 = s16in >> 12;
}
