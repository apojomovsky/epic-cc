// P6 i32 add/mul acceptance: inline add, mul_u32 (the only routine
// copy), const shifts and logic, chained through one global. One
// global and single-use statements keep main's peak at 8 bytes:
// 4 + 8 + 19 = 31 of 32 bank bytes, 1 free.
// Expected: x = 0x12345678 -> 0xBF41AAC5.
volatile unsigned long x;

void main(void) {
    x = x + 0x11111111;               // add i32: 0x23456789
    x = x * 3;                        // mul i32: 0x69D0369B
    x = x << 2;                       // shl i32 const: 0xA740DA6C
    x = (x >> 3) | 0x01234567;        // lshr i32 const + or: 0x15EB5F6F
    x = x & 0xFFFF00FF;               // and i32: 0x15EB006F
    x = x ^ 0xAAAAAAAA;               // xor i32: 0xBF41AAC5
}
