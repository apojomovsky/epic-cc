// P6 i32 variable-shift acceptance: reg-count shifts through the
// shl/lshr routine copies (const counts lower inline and prove
// nothing here). 5 globals + ~9 main peak + 10 shift frame = 24 of
// 32 bank bytes; the round-trip is lossless (top bits are 0).
// Expected: x = 0x12345678, n = 3 -> 0x12355779.
volatile unsigned long x;
volatile unsigned char n;

void main(void) {
    x = x + 0x00010101;               // setup: 0x12355779
    x = x << (n & 31);                // shl i32 variable: 0x91AABBC8
    x = x >> (n & 31);                // lshr i32 variable: 0x12355779
}
