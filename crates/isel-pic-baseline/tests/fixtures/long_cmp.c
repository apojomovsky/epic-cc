// P6 i32 sub/compare acceptance: inline sub, unsigned compares and
// const shifts. No routine copies, so RAM is easy; kept small so
// main fits one 512-word page under D-2's per-access selects.
// Expected: x = 0x12345678 -> 0x044CD55E.
volatile unsigned long x;

void main(void) {
    x = x - 0x01010101;               // sub i32: 0x11335577
    x = x + (x > 0x10000000);         // icmp ugt + zext: +1
    x = x + (x < 0x30000000);         // icmp ult + zext: +1
    x = x << 2;                       // shl i32 const: 0x44CD55E4
    x = x >> 4;                       // lshr i32 const: 0x044CD55E
}
