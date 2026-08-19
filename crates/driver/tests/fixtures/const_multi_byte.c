// Issue #3 acceptance: const (flash) tables of multi-byte elements
// (i16 / i32 / float) read through the chunked RETLW readers, plus the
// small-table (< 256-byte) element-list path. Generated fixture — the
// element lists are clang's only initializer form for multi-byte const
// arrays, so the table bytes below must decode as little-endian elements:
//   t16[i] = 0x1000 + i   ->  bytes (i, 0x10)
//   t32[i] = 0x01020304 + i*0x01010101  ->  value 0x01020304 + i*0x01010101
//            (LE bytes: value&0xFF, (value>>8)&0xFF, (value>>16)&0xFF, (value>>24)&0xFF)
//   tf[i]  = i*0.1f
// Every read uses a runtime index (clang -O1 folds constant indices into
// literals); the `& 0x7F`/`& 0x3F`/`& 1`/`& 3` masks keep indices in
// bounds. in == 290 (0x0122) gives:
//   t16[290 & 0x7F = 34]  = 0x1022   byte 68,  chunk 0
//   t16[128 + (290 & 1)]  = 0x1080   byte 256, chunk 1 (scale-2 carry!)
//   t32[290 & 0x3F = 34]  = 0x23242526  byte 136, chunk 0
//   t32[64 + (290 & 1)]   = 0x41424344  byte 256, chunk 1 (scale-4 carry!)
//   tf[290 & 0x3F = 34]   = 3.4f    byte 136, chunk 0 (f64-narrowed init 0x3FB99999A0000000 -> 0x3DCCCCCD)
//   tf[64 + (290 & 1)]    = 6.4f    byte 256, chunk 1 (f64-narrowed init)
// (t16 = 260 bytes, t32 = 400, tf = 400 — all past the 255-byte chunk
// boundary; t16s/t32s are 6/12-byte small tables taking the accumulator
// path for scale-2/4 indices.)
volatile unsigned short in;          // 0x20-0x21: 16-bit index input
volatile unsigned int out_s16;
volatile unsigned long out_s32;
volatile unsigned int out_l16;
volatile unsigned int out_l16b;
volatile unsigned long out_l32;
volatile unsigned long out_l32b;
volatile float outf;
volatile float outf2;

const unsigned short t16s[3] = { 0x1234, 0x5678, 0x9ABC };
const unsigned long t32s[3] = { 0x01020304UL, 0x05060708UL, 0x090A0B0CUL };
const unsigned short t16[130] = { 0x1000, 0x1001, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x1007, 0x1008, 0x1009, 0x100A, 0x100B, 0x100C, 0x100D, 0x100E, 0x100F, 0x1010, 0x1011, 0x1012, 0x1013, 0x1014, 0x1015, 0x1016, 0x1017, 0x1018, 0x1019, 0x101A, 0x101B, 0x101C, 0x101D, 0x101E, 0x101F, 0x1020, 0x1021, 0x1022, 0x1023, 0x1024, 0x1025, 0x1026, 0x1027, 0x1028, 0x1029, 0x102A, 0x102B, 0x102C, 0x102D, 0x102E, 0x102F, 0x1030, 0x1031, 0x1032, 0x1033, 0x1034, 0x1035, 0x1036, 0x1037, 0x1038, 0x1039, 0x103A, 0x103B, 0x103C, 0x103D, 0x103E, 0x103F, 0x1040, 0x1041, 0x1042, 0x1043, 0x1044, 0x1045, 0x1046, 0x1047, 0x1048, 0x1049, 0x104A, 0x104B, 0x104C, 0x104D, 0x104E, 0x104F, 0x1050, 0x1051, 0x1052, 0x1053, 0x1054, 0x1055, 0x1056, 0x1057, 0x1058, 0x1059, 0x105A, 0x105B, 0x105C, 0x105D, 0x105E, 0x105F, 0x1060, 0x1061, 0x1062, 0x1063, 0x1064, 0x1065, 0x1066, 0x1067, 0x1068, 0x1069, 0x106A, 0x106B, 0x106C, 0x106D, 0x106E, 0x106F, 0x1070, 0x1071, 0x1072, 0x1073, 0x1074, 0x1075, 0x1076, 0x1077, 0x1078, 0x1079, 0x107A, 0x107B, 0x107C, 0x107D, 0x107E, 0x107F, 0x1080, 0x1081 };
const unsigned long t32[100] = { 0x01020304UL, 0x02030405UL, 0x03040506UL, 0x04050607UL, 0x05060708UL, 0x06070809UL, 0x0708090AUL, 0x08090A0BUL, 0x090A0B0CUL, 0x0A0B0C0DUL, 0x0B0C0D0EUL, 0x0C0D0E0FUL, 0x0D0E0F10UL, 0x0E0F1011UL, 0x0F101112UL, 0x10111213UL, 0x11121314UL, 0x12131415UL, 0x13141516UL, 0x14151617UL, 0x15161718UL, 0x16171819UL, 0x1718191AUL, 0x18191A1BUL, 0x191A1B1CUL, 0x1A1B1C1DUL, 0x1B1C1D1EUL, 0x1C1D1E1FUL, 0x1D1E1F20UL, 0x1E1F2021UL, 0x1F202122UL, 0x20212223UL, 0x21222324UL, 0x22232425UL, 0x23242526UL, 0x24252627UL, 0x25262728UL, 0x26272829UL, 0x2728292AUL, 0x28292A2BUL, 0x292A2B2CUL, 0x2A2B2C2DUL, 0x2B2C2D2EUL, 0x2C2D2E2FUL, 0x2D2E2F30UL, 0x2E2F3031UL, 0x2F303132UL, 0x30313233UL, 0x31323334UL, 0x32333435UL, 0x33343536UL, 0x34353637UL, 0x35363738UL, 0x36373839UL, 0x3738393AUL, 0x38393A3BUL, 0x393A3B3CUL, 0x3A3B3C3DUL, 0x3B3C3D3EUL, 0x3C3D3E3FUL, 0x3D3E3F40UL, 0x3E3F4041UL, 0x3F404142UL, 0x40414243UL, 0x41424344UL, 0x42434445UL, 0x43444546UL, 0x44454647UL, 0x45464748UL, 0x46474849UL, 0x4748494AUL, 0x48494A4BUL, 0x494A4B4CUL, 0x4A4B4C4DUL, 0x4B4C4D4EUL, 0x4C4D4E4FUL, 0x4D4E4F50UL, 0x4E4F5051UL, 0x4F505152UL, 0x50515253UL, 0x51525354UL, 0x52535455UL, 0x53545556UL, 0x54555657UL, 0x55565758UL, 0x56575859UL, 0x5758595AUL, 0x58595A5BUL, 0x595A5B5CUL, 0x5A5B5C5DUL, 0x5B5C5D5EUL, 0x5C5D5E5FUL, 0x5D5E5F60UL, 0x5E5F6061UL, 0x5F606162UL, 0x60616263UL, 0x61626364UL, 0x62636465UL, 0x63646566UL, 0x64656667UL };
const float tf[100] = { 0.0f, 0.1f, 0.2f, 0.30000000000000004f, 0.4f, 0.5f, 0.6000000000000001f, 0.7000000000000001f, 0.8f, 0.9f, 1.0f, 1.1f, 1.2000000000000002f, 1.3f, 1.4000000000000001f, 1.5f, 1.6f, 1.7000000000000002f, 1.8f, 1.9000000000000001f, 2.0f, 2.1f, 2.2f, 2.3000000000000003f, 2.4000000000000004f, 2.5f, 2.6f, 2.7f, 2.8000000000000003f, 2.9000000000000004f, 3.0f, 3.1f, 3.2f, 3.3000000000000003f, 3.4000000000000004f, 3.5f, 3.6f, 3.7f, 3.8000000000000003f, 3.9000000000000004f, 4.0f, 4.1000000000000005f, 4.2f, 4.3f, 4.4f, 4.5f, 4.6000000000000005f, 4.7f, 4.800000000000001f, 4.9f, 5.0f, 5.1000000000000005f, 5.2f, 5.300000000000001f, 5.4f, 5.5f, 5.6000000000000005f, 5.7f, 5.800000000000001f, 5.9f, 6.0f, 6.1000000000000005f, 6.2f, 6.300000000000001f, 6.4f, 6.5f, 6.6000000000000005f, 6.7f, 6.800000000000001f, 6.9f, 7.0f, 7.1000000000000005f, 7.2f, 7.300000000000001f, 7.4f, 7.5f, 7.6000000000000005f, 7.7f, 7.800000000000001f, 7.9f, 8.0f, 8.1f, 8.200000000000001f, 8.3f, 8.4f, 8.5f, 8.6f, 8.700000000000001f, 8.8f, 8.9f, 9.0f, 9.1f, 9.200000000000001f, 9.3f, 9.4f, 9.5f, 9.600000000000001f, 9.700000000000001f, 9.8f, 9.9f };

void main(void) {
    out_s16 = t16s[in & 3];
    out_s32 = t32s[in & 3];
    out_l16 = t16[in & 0x7F];
    out_l16b = t16[128 + (in & 1)];
    out_l32 = t32[in & 0x3F];
    out_l32b = t32[64 + (in & 1)];
    outf = tf[in & 0x3F];
    outf2 = tf[64 + (in & 1)];
}
