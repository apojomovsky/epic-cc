// Issue #4 acceptance: runtime-length memcpy. clang -O1 emits the length
// as a zext'd i16 register (`i16 %k`), which irparse now accepts as
// `MemLen::Reg` and isel lowers to a counted 16-bit copy loop (all loop
// state in fixed common RAM — countdown 0x71/0x72, byte index 0x7E, held
// byte 0x7F — so the banking pass can never insert a BANKSEL inside the
// skip-sensitive test/branch pairs).
//
// The fixture:
//   buf2[i] = (i*0x37) & 0xFF  (initialized pattern)
//   buf1 / buf3 zeroed
//   in == 0x0A (10):
//     k  = in & 0xFF = 10          -> memcpy(buf1, buf2, k) copies 10 bytes
//     k2 = (in >> 4) = 0           -> memcpy(buf3, buf2, 0) copies nothing
//                                     (exercises the loop's zero-length guard)
//     out = buf1[9] + buf3[4]      = 0xEF + 0x00 = 0xEF
//   buf1[9] = (9*0x37) & 0xFF = 0xEF; buf3 stays all zeros.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char buf1[16];
volatile unsigned char buf2[16] = {
    0x00,0x37,0x6E,0xA5,0xDC,0x13,0x4A,0x81,
    0xB8,0xEF,0x26,0x5D,0x94,0xCB,0x02,0x39
};
volatile unsigned char buf3[16];

void main(void) {
    unsigned char k = (unsigned char)(in & 0xFF);
    unsigned char k2 = (unsigned char)((in >> 4) & 0xFF);
    __builtin_memcpy(buf1, buf2, k);   // runtime length 10 -> 10-byte loop
    __builtin_memcpy(buf3, buf2, k2);  // runtime length 0 -> guard skips
    out = (unsigned char)(buf1[9] + buf3[4]);
}
